import type { ComposerCommand } from './components/ComposerCommands'
import type { AgentContextWindowSnapshot } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useProjectSettings } from '../../config/ProjectSettingsProvider'
import { ChatComposer } from './components/ChatComposer'
import type { ChatComposerDraft, ChatSubmitOptions } from './chatTypes'
import { getNewConversationPromptKeys } from './newConversationPrompts'
import './NewConversationPage.css'

interface NewConversationPageProps {
  commands?: readonly ComposerCommand[]
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
  promptIndex: number
  skillCatalogRefreshToken?: number
}

export function NewConversationPage({
  commands,
  contextWindowIndicatorEnabled = false,
  contextWindowSnapshot,
  defaultProjectId = null,
  draft,
  onDraftChange,
  onDraftMessageChange,
  onSubmitMessage,
  permissionModeAvailability,
  promptIndex,
  skillCatalogRefreshToken
}: NewConversationPageProps) {
  const { t } = useFrontendConfig()
  const { projects } = useProjectSettings()
  const selectedProjectId = draft.projectId ?? defaultProjectId
  const selectedProject = projects.find((project) => project.id === selectedProjectId)
  const promptKeys = getNewConversationPromptKeys(promptIndex)
  const projectTitleTemplate = t('chat.projectTitle')
  const projectNamePlaceholder = '{projectName}'
  const projectNamePlaceholderIndex = projectTitleTemplate.indexOf(projectNamePlaceholder)
  const projectTitlePrefix =
    projectNamePlaceholderIndex >= 0
      ? projectTitleTemplate.slice(0, projectNamePlaceholderIndex)
      : projectTitleTemplate
  const projectTitleSuffix =
    projectNamePlaceholderIndex >= 0
      ? projectTitleTemplate.slice(projectNamePlaceholderIndex + projectNamePlaceholder.length)
      : ''

  return (
    <section className="new-conversation-page" aria-label={t('chat.newConversation')}>
      <div className="new-conversation-page__content">
        <h1>
          {selectedProject ? (
            <>
              {projectTitlePrefix}
              <span className="new-conversation-page__project-name" title={selectedProject.name}>
                {selectedProject.name}
              </span>
              {projectTitleSuffix}
            </>
          ) : (
            t(promptKeys.titleKey)
          )}
        </h1>
        <ChatComposer
          commands={commands}
          contextWindowIndicatorEnabled={contextWindowIndicatorEnabled}
          contextWindowSnapshot={contextWindowSnapshot}
          defaultProjectId={defaultProjectId}
          draft={draft}
          onDraftChange={onDraftChange}
          onDraftMessageChange={onDraftMessageChange}
          permissionModeAvailability={permissionModeAvailability}
          inputPlaceholder={selectedProject ? undefined : t(promptKeys.placeholderKey)}
          resetKey={`new:${defaultProjectId ?? 'root'}`}
          skillCatalogRefreshToken={skillCatalogRefreshToken}
          portalMenus
          showProjectSelector
          onSubmitMessage={onSubmitMessage}
        />
      </div>
    </section>
  )
}
