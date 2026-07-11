import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useProjectSettings } from '../../config/ProjectSettingsProvider'
import { ChatComposer } from './components/ChatComposer'
import type { ChatComposerDraft, ChatSubmitOptions } from './chatTypes'
import './NewConversationPage.css'

interface NewConversationPageProps {
  draft: ChatComposerDraft
  defaultProjectId?: string | null
  onDraftChange: (draft: ChatComposerDraft) => void
  onSubmitMessage: (message: string, options: ChatSubmitOptions) => void
  permissionModeAvailability: {
    custom: boolean
    full: boolean
  }
}

export function NewConversationPage({
  defaultProjectId = null,
  draft,
  onDraftChange,
  onSubmitMessage,
  permissionModeAvailability
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
          defaultProjectId={defaultProjectId}
          draft={draft}
          onDraftChange={onDraftChange}
          permissionModeAvailability={permissionModeAvailability}
          resetKey={`new:${defaultProjectId ?? 'root'}`}
          showProjectSelector
          onSubmitMessage={onSubmitMessage}
        />
      </div>
    </section>
  )
}
