// Renderer UI.
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import type { LucideIcon } from 'lucide-react'
import {
  ArrowUp,
  Check,
  ChevronDown,
  Folder,
  ImageIcon,
  Paperclip,
  Plus,
  Search,
  ShieldAlert,
  ShieldCheck,
  ShieldPlus,
  X
} from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useProjectSettings } from '../../../config/ProjectSettingsProvider'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import {
  buildAgentInputAttachments,
  composerAttachmentFromAgentAttachment,
  createComposerAttachmentsFromFiles,
  createAttachmentSummary,
  selectComposerAttachments
} from '../chatAttachments'
import type { ComposerAttachment, ComposerAttachmentKind } from '../chatAttachments'
import {
  getAttachmentBadgeLabel,
  getAttachmentExtension,
  getAttachmentIcon,
  getAttachmentTypeLabel
} from '../attachmentDisplay'
import type { ChatComposerDraft, ChatPermissionMode, ChatSubmitOptions } from '../chatTypes'
import './ChatComposer.css'

interface PermissionOption {
  id: ChatPermissionMode
  labelKey: TranslationKey
  icon: LucideIcon
}

const PERMISSION_OPTIONS: PermissionOption[] = [
  { id: 'default', labelKey: 'chat.defaultPermission', icon: ShieldPlus },
  { id: 'full', labelKey: 'chat.fullPermission', icon: ShieldAlert },
  { id: 'custom', labelKey: 'chat.customPermission', icon: ShieldCheck }
]

const TEXTAREA_MAX_HEIGHT = 220

interface ChatComposerProps {
  draft: ChatComposerDraft
  defaultProjectId?: string | null
  isGenerating?: boolean
  onDraftChange: (draft: ChatComposerDraft) => void
  onSubmitMessage?: (message: string, options: ChatSubmitOptions) => void
  onStopGenerating?: () => void
  permissionModeAvailability?: {
    custom: boolean
    full: boolean
  }
  showProjectSelector?: boolean
}

export function ChatComposer({
  draft,
  defaultProjectId = null,
  isGenerating = false,
  onDraftChange,
  onSubmitMessage,
  onStopGenerating,
  permissionModeAvailability = { custom: true, full: true },
  showProjectSelector = false
}: ChatComposerProps) {
  const { t } = useFrontendConfig()
  const { enabledModels } = useModelSettings()
  const { projects, selectProjectDirectory } = useProjectSettings()
  const draftRef = useRef(draft)
  const composerRef = useRef<HTMLFormElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const isComposingRef = useRef(false)
  const lastCompositionEndAtRef = useRef(0)
  const attachmentPickerRef = useRef<HTMLDivElement>(null)
  const permissionPickerRef = useRef<HTMLDivElement>(null)
  const modelPickerRef = useRef<HTMLDivElement>(null)
  const projectPickerRef = useRef<HTMLDivElement>(null)
  const [isAttachmentMenuOpen, setIsAttachmentMenuOpen] = useState(false)
  const [isPermissionMenuOpen, setIsPermissionMenuOpen] = useState(false)
  const [isModelMenuOpen, setIsModelMenuOpen] = useState(false)
  const [isProjectMenuOpen, setIsProjectMenuOpen] = useState(false)
  const [isFileDragActive, setIsFileDragActive] = useState(false)
  const [attachmentError, setAttachmentError] = useState<string | null>(null)
  const [projectSearch, setProjectSearch] = useState('')
  const message = draft.message
  const permissionOptions = useMemo(
    () =>
      PERMISSION_OPTIONS.filter(
        (option) =>
          option.id === 'default' ||
          (option.id === 'full' && permissionModeAvailability.full) ||
          (option.id === 'custom' && permissionModeAvailability.custom)
      ),
    [permissionModeAvailability.custom, permissionModeAvailability.full]
  )
  const permissionMode = permissionOptions.some((option) => option.id === draft.permissionMode)
    ? draft.permissionMode
    : 'default'
  const selectedProjectId = draft.projectId
  const selectedModelId = draft.modelId
  const attachments = useMemo(
    () => draft.attachments.map(composerAttachmentFromAgentAttachment),
    [draft.attachments]
  )
  const selectedPermission =
    permissionOptions.find((option) => option.id === permissionMode) ?? PERMISSION_OPTIONS[0]
  const SelectedPermissionIcon = selectedPermission.icon
  const selectedModel = useMemo(() => {
    return enabledModels.find((model) => model.id === selectedModelId) ?? enabledModels[0]
  }, [enabledModels, selectedModelId])
  const selectedProject = projects.find((project) => project.id === selectedProjectId)
  const filteredProjects = projects.filter((project) =>
    project.name.toLowerCase().includes(projectSearch.trim().toLowerCase())
  )
  const hasImageAttachment = attachments.some((attachment) => attachment.kind === 'image')
  const hasUnsupportedImageAttachment = hasImageAttachment && !selectedModel?.supportsImage
  const hasSendableContent = message.trim().length > 0 || attachments.length > 0
  const canSend = hasSendableContent && !hasUnsupportedImageAttachment && Boolean(selectedModel)
  const submitButtonState = isGenerating ? 'stop' : canSend ? 'ready' : 'disabled'
  const isConfirmingImeInput = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    const nativeEvent = event.nativeEvent
    const keyCode = 'keyCode' in nativeEvent ? nativeEvent.keyCode : 0
    return (
      isComposingRef.current ||
      nativeEvent.isComposing ||
      keyCode === 229 ||
      Date.now() - lastCompositionEndAtRef.current < 120
    )
  }

  useDismissOnOutsidePointer(attachmentPickerRef, isAttachmentMenuOpen, () =>
    setIsAttachmentMenuOpen(false)
  )
  useDismissOnOutsidePointer(permissionPickerRef, isPermissionMenuOpen, () =>
    setIsPermissionMenuOpen(false)
  )
  useDismissOnOutsidePointer(modelPickerRef, isModelMenuOpen, () => setIsModelMenuOpen(false))
  useDismissOnOutsidePointer(projectPickerRef, isProjectMenuOpen, () => setIsProjectMenuOpen(false))

  useEffect(() => {
    draftRef.current = draft
  }, [draft])

  const updateDraft = (patch: Partial<ChatComposerDraft>) => {
    const currentDraft = draftRef.current
    const nextDraft = {
      ...currentDraft,
      ...patch,
      updatedAt: Date.now()
    }
    draftRef.current = nextDraft
    onDraftChange({
      ...nextDraft
    })
  }

  useEffect(() => {
    if (draft.permissionMode !== permissionMode) {
      updateDraft({ permissionMode })
    }
  }, [draft.permissionMode, permissionMode])

  const appendAttachments = (nextAttachments: ComposerAttachment[]) => {
    if (nextAttachments.length === 0) return

    updateDraft({
      attachments: [...draftRef.current.attachments, ...buildAgentInputAttachments(nextAttachments)]
    })
  }

  useEffect(() => {
    if (showProjectSelector && defaultProjectId !== null && defaultProjectId !== draft.projectId) {
      updateDraft({ projectId: defaultProjectId })
    }
  }, [defaultProjectId, showProjectSelector])

  useEffect(() => {
    if (draft.projectId && !projects.some((project) => project.id === draft.projectId)) {
      updateDraft({ projectId: null })
    }
  }, [draft.projectId, projects])

  useEffect(() => {
    if (!selectedModel && enabledModels.length > 0) {
      updateDraft({ modelId: enabledModels[0].id })
    }
  }, [enabledModels, selectedModel])

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    const nextHeight = Math.min(textarea.scrollHeight, TEXTAREA_MAX_HEIGHT)
    textarea.style.height = `${nextHeight}px`
    textarea.style.overflowY = textarea.scrollHeight > TEXTAREA_MAX_HEIGHT ? 'auto' : 'hidden'
  }, [message])

  const submitMessage = async () => {
    if (isGenerating || !canSend) return

    const trimmedMessage = message.trim()
    const attachmentSummary = createAttachmentSummary(attachments)
    const messageContent = [trimmedMessage, attachmentSummary].filter(Boolean).join('\n\n')
    let inputAttachments: ChatSubmitOptions['attachments']

    try {
      inputAttachments = await buildAgentInputAttachments(attachments)
      setAttachmentError(null)
    } catch (error) {
      setAttachmentError(error instanceof Error ? error.message : String(error))
      return
    }

    onSubmitMessage?.(messageContent, {
      attachments: inputAttachments,
      modelId: selectedModel?.id ?? selectedModelId,
      permissionMode,
      projectId: selectedProject?.id ?? null
    })
    updateDraft({
      message: '',
      attachments: [],
      modelId: selectedModel?.id ?? selectedModelId,
      permissionMode,
      projectId: selectedProject?.id ?? null
    })
    setIsAttachmentMenuOpen(false)
    setIsPermissionMenuOpen(false)
    setIsModelMenuOpen(false)
    setIsProjectMenuOpen(false)
  }

  const removeAttachment = (attachmentId: string) => {
    updateDraft({
      attachments: draftRef.current.attachments.filter(
        (attachment) => attachment.id !== attachmentId
      )
    })
  }

  const addAttachments = async (kind: ComposerAttachmentKind) => {
    let nextAttachments: ComposerAttachment[]
    try {
      nextAttachments = await selectComposerAttachments(kind)
      setAttachmentError(null)
    } catch (error) {
      setAttachmentError(error instanceof Error ? error.message : String(error))
      setIsAttachmentMenuOpen(false)
      return
    }

    if (nextAttachments.length === 0) {
      setIsAttachmentMenuOpen(false)
      return
    }

    appendAttachments(nextAttachments)
    setAttachmentError(null)
    setIsAttachmentMenuOpen(false)
  }

  const addDroppedOrPastedFiles = async (files: FileList | File[]) => {
    if (files.length === 0) return

    try {
      const nextAttachments = await createComposerAttachmentsFromFiles(files)
      appendAttachments(nextAttachments)
      setAttachmentError(null)
    } catch (error) {
      setAttachmentError(error instanceof Error ? error.message : String(error))
    }
  }

  const handleSelectProjectDirectory = async () => {
    const project = await selectProjectDirectory()
    if (!project) return

    updateDraft({ projectId: project.id })
    setProjectSearch('')
    setIsProjectMenuOpen(false)
  }

  return (
    <form
      ref={composerRef}
      className="chat-composer"
      data-drag-active={isFileDragActive || undefined}
      data-submit-state={submitButtonState}
      aria-label={t('chat.composer')}
      onSubmit={(event) => {
        event.preventDefault()
        submitMessage()
      }}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault()
          setIsFileDragActive(true)
        }
      }}
      onDragLeave={(event) => {
        const nextTarget = event.relatedTarget
        if (nextTarget instanceof Node && event.currentTarget.contains(nextTarget)) return
        setIsFileDragActive(false)
      }}
      onDrop={(event) => {
        if (event.dataTransfer.files.length === 0) return
        event.preventDefault()
        setIsFileDragActive(false)
        void addDroppedOrPastedFiles(event.dataTransfer.files)
      }}
      onPaste={(event) => {
        if (event.clipboardData.files.length === 0) return
        event.preventDefault()
        void addDroppedOrPastedFiles(event.clipboardData.files)
      }}
    >
      {attachments.length > 0 && (
        <div className="chat-composer__attachments" aria-label={t('chat.attachments')}>
          {attachments.map((attachment) => {
            const extension = getAttachmentExtension(attachment.name)
            const AttachmentIcon = getAttachmentIcon(attachment.kind, extension)
            const badgeLabel = getAttachmentBadgeLabel(extension)
            const typeLabel = getAttachmentTypeLabel(attachment)
            const isImagePreview = attachment.kind === 'image' && Boolean(attachment.previewUrl)

            return (
              <div className="composer-attachment" data-kind={attachment.kind} key={attachment.id}>
                {isImagePreview ? (
                  <img
                    className="composer-attachment__thumbnail"
                    src={attachment.previewUrl}
                    alt={attachment.name}
                  />
                ) : (
                  <>
                    <div className="composer-attachment__icon" aria-hidden="true">
                      {badgeLabel ? (
                        <span className="composer-attachment__language-badge">{badgeLabel}</span>
                      ) : (
                        <AttachmentIcon />
                      )}
                    </div>
                    <div className="composer-attachment__details">
                      <span className="composer-attachment__name">{attachment.name}</span>
                      <span className="composer-attachment__type">{typeLabel}</span>
                    </div>
                  </>
                )}
                <button
                  type="button"
                  className="composer-attachment__remove"
                  aria-label={`${t('chat.removeAttachment')} ${attachment.name}`}
                  onClick={() => removeAttachment(attachment.id)}
                >
                  <X aria-hidden="true" />
                </button>
              </div>
            )
          })}
        </div>
      )}

      <textarea
        ref={textareaRef}
        value={message}
        placeholder={t('chat.inputPlaceholder')}
        aria-label={t('chat.inputAria')}
        rows={1}
        onChange={(event) => updateDraft({ message: event.target.value })}
        onCompositionStart={() => {
          isComposingRef.current = true
        }}
        onCompositionEnd={() => {
          isComposingRef.current = false
          lastCompositionEndAtRef.current = Date.now()
        }}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' || event.shiftKey || isConfirmingImeInput(event)) return

          event.preventDefault()
          if (!isGenerating) {
            submitMessage()
          }
        }}
      />

      {hasUnsupportedImageAttachment && (
        <p className="chat-composer__warning">{t('chat.unsupportedImageWarning')}</p>
      )}
      {attachmentError && <p className="chat-composer__warning">{attachmentError}</p>}

      <div className="chat-composer__toolbar">
        <div className="composer-add-picker" ref={attachmentPickerRef}>
          <button
            type="button"
            className="composer-icon-button"
            aria-haspopup="menu"
            aria-expanded={isAttachmentMenuOpen}
            aria-label={t('chat.addContext')}
            onClick={() => {
              setIsAttachmentMenuOpen((open) => !open)
              setIsPermissionMenuOpen(false)
              setIsModelMenuOpen(false)
              setIsProjectMenuOpen(false)
            }}
          >
            <Plus aria-hidden="true" />
          </button>

          {isAttachmentMenuOpen && (
            <div className="composer-add-menu" role="menu" aria-label={t('chat.addMenuTitle')}>
              <p>{t('chat.addMenuTitle')}</p>
              <button type="button" role="menuitem" onClick={() => void addAttachments('file')}>
                <Paperclip aria-hidden="true" />
                <span>{t('chat.addFile')}</span>
              </button>
              <button type="button" role="menuitem" onClick={() => void addAttachments('image')}>
                <ImageIcon aria-hidden="true" />
                <span>{t('chat.addImage')}</span>
              </button>
            </div>
          )}
        </div>

        <div className="composer-permission-picker" ref={permissionPickerRef}>
          <button
            type="button"
            className="composer-permission-button"
            data-permission={selectedPermission.id}
            aria-haspopup="listbox"
            aria-expanded={isPermissionMenuOpen}
            aria-label={`${t('chat.permission')}：${t(selectedPermission.labelKey)}`}
            onClick={() => {
              setIsAttachmentMenuOpen(false)
              setIsModelMenuOpen(false)
              setIsPermissionMenuOpen((open) => !open)
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                setIsPermissionMenuOpen(false)
              }
            }}
          >
            <SelectedPermissionIcon aria-hidden="true" />
            <span>{t(selectedPermission.labelKey)}</span>
            <ChevronDown aria-hidden="true" />
          </button>

          {isPermissionMenuOpen && (
            <div
              className="composer-permission-menu"
              role="listbox"
              aria-label={t('chat.selectPermission')}
            >
              {permissionOptions.map((option) => {
                const OptionIcon = option.icon
                const isSelected = option.id === permissionMode

                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className="composer-permission-option"
                    data-permission={option.id}
                    data-selected={isSelected || undefined}
                    key={option.id}
                    onClick={() => {
                      updateDraft({ permissionMode: option.id })
                      setIsPermissionMenuOpen(false)
                    }}
                  >
                    <OptionIcon aria-hidden="true" />
                    <span>{t(option.labelKey)}</span>
                    {isSelected && (
                      <Check className="composer-permission-option__check" aria-hidden="true" />
                    )}
                  </button>
                )
              })}
            </div>
          )}
        </div>

        <span className="chat-composer__spacer" />

        <div className="composer-model-picker" ref={modelPickerRef}>
          <button
            type="button"
            className="composer-model-button"
            aria-haspopup="listbox"
            aria-expanded={isModelMenuOpen}
            aria-label={t('chat.selectModel')}
            onClick={() => {
              setIsAttachmentMenuOpen(false)
              setIsPermissionMenuOpen(false)
              setIsModelMenuOpen((open) => !open)
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                setIsModelMenuOpen(false)
              }
            }}
          >
            <span>
              {selectedModel?.shortName ?? selectedModel?.displayName ?? t('chat.noEnabledModels')}
            </span>
            <ChevronDown aria-hidden="true" />
          </button>

          {isModelMenuOpen && (
            <div className="composer-model-menu" role="listbox" aria-label={t('chat.selectModel')}>
              {enabledModels.length === 0 ? (
                <span className="composer-model-empty">{t('chat.noEnabledModels')}</span>
              ) : (
                enabledModels.map((model) => {
                  const isSelected = model.id === selectedModel?.id

                  return (
                    <button
                      type="button"
                      role="option"
                      aria-selected={isSelected}
                      className="composer-model-option"
                      data-selected={isSelected || undefined}
                      key={model.id}
                      onClick={() => {
                        updateDraft({ modelId: model.id })
                        setIsModelMenuOpen(false)
                      }}
                    >
                      <span className="composer-model-option__name">
                        {model.shortName ?? model.displayName}
                      </span>
                      <span
                        className="composer-model-option__capability"
                        data-supported={model.supportsImage || undefined}
                      >
                        {model.supportsImage ? t('configuration.image') : t('configuration.text')}
                      </span>
                    </button>
                  )
                })
              )}
            </div>
          )}
        </div>

        <button
          type={isGenerating ? 'button' : 'submit'}
          className="composer-submit-button"
          data-state={submitButtonState}
          disabled={!isGenerating && !canSend}
          aria-label={isGenerating ? t('chat.stop') : t('chat.send')}
          onClick={() => {
            if (isGenerating) {
              onStopGenerating?.()
            }
          }}
        >
          {isGenerating ? (
            <span className="composer-stop-square" aria-hidden="true" />
          ) : (
            <ArrowUp aria-hidden="true" />
          )}
        </button>
      </div>

      {showProjectSelector && (
        <div className="chat-composer__project-row">
          <div className="composer-project-picker" ref={projectPickerRef}>
            <button
              className="composer-project-button"
              type="button"
              aria-haspopup="listbox"
              aria-expanded={isProjectMenuOpen}
              onClick={() => {
                setIsAttachmentMenuOpen(false)
                setIsPermissionMenuOpen(false)
                setIsModelMenuOpen(false)
                setIsProjectMenuOpen((open) => !open)
              }}
            >
              <Folder aria-hidden="true" />
              <span>{selectedProject?.name ?? t('project.chooseProject')}</span>
            </button>

            {isProjectMenuOpen && (
              <div
                className="composer-project-menu"
                role="listbox"
                aria-label={t('project.chooseProject')}
              >
                <label className="composer-project-menu__search">
                  <Search aria-hidden="true" />
                  <input
                    value={projectSearch}
                    placeholder={t('project.searchProject')}
                    onChange={(event) => setProjectSearch(event.target.value)}
                  />
                </label>

                <div className="composer-project-menu__items">
                  {filteredProjects.map((project) => {
                    const isSelected = project.id === selectedProjectId

                    return (
                      <button
                        className="composer-project-option"
                        data-selected={isSelected || undefined}
                        type="button"
                        role="option"
                        aria-selected={isSelected}
                        key={project.id}
                        onClick={() => {
                          updateDraft({ projectId: project.id })
                          setProjectSearch('')
                          setIsProjectMenuOpen(false)
                        }}
                      >
                        <Folder aria-hidden="true" />
                        <span>{project.name}</span>
                        {isSelected && <Check aria-hidden="true" />}
                      </button>
                    )
                  })}
                </div>

                <div className="composer-project-menu__divider" />

                <button
                  className="composer-project-command"
                  type="button"
                  onClick={() => {
                    void handleSelectProjectDirectory()
                  }}
                >
                  <Plus aria-hidden="true" />
                  <span>{t('project.newProject')}</span>
                </button>

                <button
                  className="composer-project-command"
                  type="button"
                  onClick={() => {
                    updateDraft({ projectId: null })
                    setProjectSearch('')
                    setIsProjectMenuOpen(false)
                  }}
                >
                  <X aria-hidden="true" />
                  <span>{t('project.noProject')}</span>
                </button>
              </div>
            )}
          </div>
        </div>
      )}
    </form>
  )
}
