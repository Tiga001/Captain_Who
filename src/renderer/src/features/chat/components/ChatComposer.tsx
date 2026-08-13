import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import type { AgentContextWindowSnapshot, SkillDescriptor } from '@mycopilot/protocol'
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
  Sparkles,
  ShieldAlert,
  ShieldCheck,
  ShieldPlus,
  X
} from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useProjectSettings } from '../../../config/ProjectSettingsProvider'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import {
  buildAgentInputAttachments,
  composerAttachmentFromAgentAttachment,
  createComposerAttachmentsFromFiles,
  createAttachmentSummary,
  selectComposerAttachments,
  stripAttachmentSummary
} from '../chatAttachments'
import type { ComposerAttachment, ComposerAttachmentKind } from '../chatAttachments'
import {
  getAttachmentBadgeLabel,
  getAttachmentExtension,
  getAttachmentIcon,
  getAttachmentTypeLabel
} from '../attachmentDisplay'
import type {
  ChatComposerDraft,
  ChatPermissionMode,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../chatTypes'
import {
  filterSkillDescriptors,
  matchSkillSelection,
  removeSkillSelection,
  retainGlobalSkillSelections,
  toggleSkillSelection,
  updateSkillSelectionRevision
} from '../../skills/skillSelection'
import { useSkillCatalog } from '../../skills/useSkillCatalog'
import { ContextWindowIndicator } from './ContextWindowIndicator'
import { ComposerSelectedSkills, ComposerSkillPicker } from './ComposerSkillPicker'
import { GuidanceQueue } from './GuidanceQueue'
import { useImagePreview } from './ImagePreview'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import './ChatComposer.css'
import './GuidanceQueue.css'

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
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  draft: ChatComposerDraft
  defaultProjectId?: string | null
  canGuideQueuedMessages?: boolean
  isGenerating?: boolean
  isModelTransitionRunning?: boolean
  messageSyncKey?: string
  onDraftChange: (draft: ChatComposerDraft) => void
  onDraftMessageChange?: (draft: ChatComposerDraft) => void
  onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
  onOpenQueuedMessageInSideChat?: (message: ChatQueuedMessage) => void
  onSubmitMessage?: (
    message: string,
    options: ChatSubmitOptions
  ) => boolean | void | Promise<boolean | void>
  onStopGenerating?: () => void
  permissionModeAvailability?: {
    custom: boolean
    full: boolean
  }
  resetKey?: string
  skillCatalogRefreshToken?: number
  showProjectSelector?: boolean
}

export function ChatComposer({
  contextWindowIndicatorEnabled = false,
  contextWindowSnapshot,
  draft,
  defaultProjectId = null,
  canGuideQueuedMessages = false,
  isGenerating = false,
  isModelTransitionRunning = false,
  messageSyncKey,
  onDraftChange,
  onDraftMessageChange,
  onGuideQueuedMessage,
  onOpenQueuedMessageInSideChat,
  onSubmitMessage,
  onStopGenerating,
  permissionModeAvailability = { custom: true, full: true },
  resetKey,
  skillCatalogRefreshToken = 0,
  showProjectSelector = false
}: ChatComposerProps) {
  const { t } = useFrontendConfig()
  const { enabledModels } = useModelSettings()
  const { projects, selectProjectDirectory } = useProjectSettings()
  const openImagePreview = useImagePreview()
  const draftRef = useRef(draft)
  const composerRef = useRef<HTMLFormElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const isComposingRef = useRef(false)
  const lastCompositionEndAtRef = useRef(0)
  const attachmentPickerRef = useRef<HTMLDivElement>(null)
  const attachmentTriggerRef = useRef<HTMLButtonElement>(null)
  const permissionPickerRef = useRef<HTMLDivElement>(null)
  const projectPickerRef = useRef<HTMLDivElement>(null)
  const [isAttachmentMenuOpen, setIsAttachmentMenuOpen] = useState(false)
  const [isSkillMenuOpen, setIsSkillMenuOpen] = useState(false)
  const [isPermissionMenuOpen, setIsPermissionMenuOpen] = useState(false)
  const [isFullPermissionConfirmationOpen, setIsFullPermissionConfirmationOpen] = useState(false)
  const [isProjectMenuOpen, setIsProjectMenuOpen] = useState(false)
  const [isFileDragActive, setIsFileDragActive] = useState(false)
  const [attachmentError, setAttachmentError] = useState<string | null>(null)
  const [projectSearch, setProjectSearch] = useState('')
  const [skillSearch, setSkillSearch] = useState('')
  const [message, setMessage] = useState(draft.message)
  const previousSkillScopeRef = useRef({ projectId: draft.projectId, resetKey })
  const previousResetKeyRef = useRef(resetKey)
  const previousMessageSyncKeyRef = useRef(messageSyncKey)
  const lastExternalMessageRef = useRef(draft.message)
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
  const skillCatalogEnabled = isSkillMenuOpen || draft.skills.length > 0
  const skillCatalogRefreshKey = `${skillCatalogRefreshToken}\u0002${
    isSkillMenuOpen
      ? 'picker-open'
      : draft.skills.map((selection) => `${selection.id}\u0000${selection.revision}`).join('\u0001')
  }`
  const { refresh: refreshSkillCatalog, state: skillCatalogState } = useSkillCatalog(
    selectedProjectId,
    skillCatalogEnabled,
    skillCatalogRefreshKey
  )
  const skillCatalog =
    skillCatalogState.status === 'ready' && skillCatalogState.projectId === selectedProjectId
      ? skillCatalogState.output
      : undefined
  const skillCatalogDescriptors = skillCatalog
    ? filterSkillDescriptors(skillCatalog.skills, '')
    : []
  const hasStaleSkillSelection = Boolean(
    skillCatalog &&
    draft.skills.some(
      (selection) => matchSkillSelection(selection, skillCatalogDescriptors).status === 'stale'
    )
  )
  const hasUnavailableSkillSelection = Boolean(
    skillCatalog &&
    !skillCatalog.truncated &&
    draft.skills.some(
      (selection) =>
        matchSkillSelection(selection, skillCatalogDescriptors).status === 'unavailable'
    )
  )
  const filteredProjects = projects.filter((project) =>
    project.name.toLowerCase().includes(projectSearch.trim().toLowerCase())
  )
  const hasImageAttachment = attachments.some((attachment) => attachment.kind === 'image')
  const hasUnsupportedImageAttachment = hasImageAttachment && !selectedModel?.supportsImage
  const hasSendableContent = message.trim().length > 0 || attachments.length > 0
  const hasInvalidSkillSelection = hasStaleSkillSelection || hasUnavailableSkillSelection
  const canSend =
    hasSendableContent &&
    !hasUnsupportedImageAttachment &&
    !hasInvalidSkillSelection &&
    !isModelTransitionRunning &&
    Boolean(selectedModel)
  const submitButtonState = isModelTransitionRunning
    ? 'disabled'
    : isGenerating
      ? canSend
        ? 'ready'
        : 'stop'
      : canSend
        ? 'ready'
        : 'disabled'
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

  useDismissOnOutsidePointer(
    attachmentPickerRef,
    isAttachmentMenuOpen || isSkillMenuOpen,
    useCallback(() => {
      setIsAttachmentMenuOpen(false)
      setIsSkillMenuOpen(false)
    }, [])
  )
  useDismissOnOutsidePointer(permissionPickerRef, isPermissionMenuOpen, () =>
    setIsPermissionMenuOpen(false)
  )
  useDismissOnOutsidePointer(projectPickerRef, isProjectMenuOpen, () => setIsProjectMenuOpen(false))

  useEffect(() => {
    if (previousResetKeyRef.current !== resetKey) {
      previousResetKeyRef.current = resetKey
      previousMessageSyncKeyRef.current = messageSyncKey
      lastExternalMessageRef.current = draft.message
      setMessage(draft.message)
      draftRef.current = draft
      return
    }

    // Message keystrokes are persisted through a ref-only fast path so the whole shell does not
    // rerender on every character. A committed user message is the explicit signal that a later
    // parent draft (including an empty one after a deferred provider transition) is authoritative.
    // Without this key, empty -> typed locally -> empty externally is indistinguishable from a
    // stale parent render because both parent values are the same empty string.
    if (previousMessageSyncKeyRef.current !== messageSyncKey) {
      previousMessageSyncKeyRef.current = messageSyncKey
      lastExternalMessageRef.current = draft.message
      setMessage(draft.message)
      draftRef.current = draft
      return
    }

    if (draft.message !== lastExternalMessageRef.current) {
      lastExternalMessageRef.current = draft.message
      setMessage(draft.message)
      draftRef.current = draft
      return
    }

    draftRef.current = {
      ...draft,
      message
    }
  }, [draft, message, messageSyncKey, resetKey])

  useEffect(() => {
    if (!isModelTransitionRunning) return
    setIsAttachmentMenuOpen(false)
    setIsSkillMenuOpen(false)
    setIsPermissionMenuOpen(false)
    setIsProjectMenuOpen(false)
  }, [isModelTransitionRunning])

  useEffect(() => {
    setAttachmentError(null)
    setIsAttachmentMenuOpen(false)
    setIsSkillMenuOpen(false)
    setIsFullPermissionConfirmationOpen(false)
    setSkillSearch('')
    setIsFileDragActive(false)
  }, [resetKey])

  const updateDraftMessage = useCallback(
    (nextMessage: string) => {
      const nextDraft = {
        ...draftRef.current,
        message: nextMessage,
        updatedAt: Date.now()
      }
      setMessage(nextMessage)
      draftRef.current = nextDraft
      onDraftMessageChange?.({ ...nextDraft })
    },
    [onDraftMessageChange]
  )

  const updateDraft = useCallback(
    (patch: Partial<ChatComposerDraft>) => {
      const currentDraft = {
        ...draftRef.current,
        message
      }
      const nextDraft = {
        ...currentDraft,
        ...patch,
        updatedAt: Date.now()
      }
      if (patch.message !== undefined) {
        setMessage(patch.message)
      }
      draftRef.current = nextDraft
      onDraftChange({
        ...nextDraft
      })
    },
    [message, onDraftChange]
  )

  useEffect(() => {
    if (draft.permissionMode !== permissionMode) {
      updateDraft({ permissionMode })
    }
  }, [draft.permissionMode, permissionMode, updateDraft])

  useEffect(() => {
    if (!permissionModeAvailability.full || permissionMode === 'full') {
      setIsFullPermissionConfirmationOpen(false)
    }
  }, [permissionMode, permissionModeAvailability.full])

  useEffect(() => {
    const previousScope = previousSkillScopeRef.current
    previousSkillScopeRef.current = { projectId: draft.projectId, resetKey }

    const scopeChanged = previousScope.resetKey !== resetKey
    const projectChanged = previousScope.projectId !== draft.projectId
    const shouldValidateProjectlessDraft = draft.projectId === null
    if ((scopeChanged && !shouldValidateProjectlessDraft) || draft.skills.length === 0) return
    if (!projectChanged && !shouldValidateProjectlessDraft) return

    const retainedSkills = retainGlobalSkillSelections(draft.skills)
    if (retainedSkills.length !== draft.skills.length) {
      updateDraft({ skills: retainedSkills })
    }
  }, [draft.projectId, draft.skills, resetKey, updateDraft])

  const appendAttachments = (nextAttachments: ComposerAttachment[]) => {
    if (nextAttachments.length === 0) return

    updateDraft({
      attachments: [...draftRef.current.attachments, ...buildAgentInputAttachments(nextAttachments)]
    })
  }

  useEffect(() => {
    if (showProjectSelector && defaultProjectId !== null && defaultProjectId !== draft.projectId) {
      updateDraft({
        projectId: defaultProjectId,
        skills: retainGlobalSkillSelections(draftRef.current.skills)
      })
    }
  }, [defaultProjectId, draft.projectId, showProjectSelector, updateDraft])

  useEffect(() => {
    if (draft.projectId && !projects.some((project) => project.id === draft.projectId)) {
      updateDraft({
        projectId: null,
        skills: retainGlobalSkillSelections(draftRef.current.skills)
      })
    }
  }, [draft.projectId, projects, updateDraft])

  useEffect(() => {
    if (!selectedModel && enabledModels.length > 0) {
      updateDraft({ modelId: enabledModels[0].id })
    }
  }, [enabledModels, selectedModel, updateDraft])

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    const nextHeight = Math.min(textarea.scrollHeight, TEXTAREA_MAX_HEIGHT)
    textarea.style.height = `${nextHeight}px`
    textarea.style.overflowY = textarea.scrollHeight > TEXTAREA_MAX_HEIGHT ? 'auto' : 'hidden'
  }, [message])

  const submitMessage = async () => {
    if (!canSend) return

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

    const submitOptions: ChatSubmitOptions = {
      attachments: inputAttachments,
      modelId: selectedModel?.id ?? selectedModelId,
      permissionMode,
      projectId: selectedProject?.id ?? null,
      skills: [...draftRef.current.skills]
    }

    if (isGenerating) {
      const createdAt = Date.now()
      updateDraft({
        message: '',
        attachments: [],
        queuedMessages: [
          ...draftRef.current.queuedMessages,
          {
            id: `queued-message-${createdAt}-${Math.random().toString(36).slice(2, 8)}`,
            clientMessageId: `guidance-${createdAt}-${Math.random().toString(36).slice(2, 10)}`,
            content: messageContent,
            attachments: inputAttachments ?? [],
            modelId: submitOptions.modelId,
            permissionMode: submitOptions.permissionMode,
            projectId: submitOptions.projectId,
            skills: submitOptions.skills,
            status: 'pending',
            createdAt
          }
        ]
      })
      setIsAttachmentMenuOpen(false)
      setIsSkillMenuOpen(false)
      return
    }

    const accepted = await onSubmitMessage?.(messageContent, submitOptions)
    if (accepted === false) return
    updateDraft({
      message: '',
      attachments: [],
      skills: [],
      modelId: selectedModel?.id ?? selectedModelId,
      permissionMode,
      projectId: selectedProject?.id ?? null
    })
    setIsAttachmentMenuOpen(false)
    setIsSkillMenuOpen(false)
    setIsPermissionMenuOpen(false)
    setIsProjectMenuOpen(false)
  }

  const removeAttachment = (attachmentId: string) => {
    updateDraft({
      attachments: draftRef.current.attachments.filter(
        (attachment) => attachment.id !== attachmentId
      )
    })
  }

  const deleteQueuedMessage = (messageId: string) => {
    updateDraft({
      queuedMessages: draftRef.current.queuedMessages.filter((message) => message.id !== messageId)
    })
  }

  const editQueuedMessage = (messageId: string) => {
    const queuedMessage = draftRef.current.queuedMessages.find(
      (message) => message.id === messageId
    )
    if (!queuedMessage || queuedMessage.status === 'submitting') return

    updateDraft({
      message: stripAttachmentSummary(queuedMessage.content, queuedMessage.attachments),
      attachments: queuedMessage.attachments,
      queuedMessages: draftRef.current.queuedMessages.filter((message) => message.id !== messageId)
    })
    window.requestAnimationFrame(() => textareaRef.current?.focus())
  }

  const moveQueuedMessage = (messageId: string, targetMessageId: string) => {
    const queuedMessages = [...draftRef.current.queuedMessages]
    const sourceIndex = queuedMessages.findIndex((message) => message.id === messageId)
    const targetIndex = queuedMessages.findIndex((message) => message.id === targetMessageId)
    if (sourceIndex === -1 || targetIndex === -1 || sourceIndex === targetIndex) return

    const [message] = queuedMessages.splice(sourceIndex, 1)
    queuedMessages.splice(targetIndex, 0, message)
    updateDraft({ queuedMessages })
  }

  const removeSkill = (skillId: string) => {
    updateDraft({
      skills: removeSkillSelection(draftRef.current.skills, skillId)
    })
  }

  const selectPermissionMode = (nextPermissionMode: ChatPermissionMode) => {
    setIsPermissionMenuOpen(false)
    if (nextPermissionMode === permissionMode) return

    if (nextPermissionMode === 'full') {
      setIsFullPermissionConfirmationOpen(true)
      return
    }

    updateDraft({ permissionMode: nextPermissionMode })
  }

  const toggleSkill = (skill: SkillDescriptor) => {
    updateDraft({
      skills: toggleSkillSelection(draftRef.current.skills, skill)
    })
  }

  const useLatestSkill = (skill: SkillDescriptor) => {
    updateDraft({
      skills: updateSkillSelectionRevision(draftRef.current.skills, skill)
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

    updateDraft({
      projectId: project.id,
      skills:
        draftRef.current.projectId === project.id
          ? draftRef.current.skills
          : retainGlobalSkillSelections(draftRef.current.skills)
    })
    setProjectSearch('')
    setIsProjectMenuOpen(false)
  }

  return (
    <div className="chat-composer-shell">
      <GuidanceQueue
        guideEnabled={canGuideQueuedMessages}
        messages={draft.queuedMessages}
        onDelete={deleteQueuedMessage}
        onEdit={editQueuedMessage}
        onGuide={(queuedMessage) => onGuideQueuedMessage?.(queuedMessage)}
        onMove={moveQueuedMessage}
        onOpenSideChat={(queuedMessage) => onOpenQueuedMessageInSideChat?.(queuedMessage)}
      />
      <form
        ref={composerRef}
        className="chat-composer"
        data-drag-active={isFileDragActive || undefined}
        data-submit-state={submitButtonState}
        aria-label={t('chat.composer')}
        onSubmit={(event) => {
          event.preventDefault()
          if (isModelTransitionRunning) return
          void submitMessage()
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
          if (isModelTransitionRunning) return
          if (event.dataTransfer.files.length === 0) return
          event.preventDefault()
          setIsFileDragActive(false)
          void addDroppedOrPastedFiles(event.dataTransfer.files)
        }}
        onPaste={(event) => {
          if (isModelTransitionRunning) return
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
                <div
                  className="composer-attachment"
                  data-kind={attachment.kind}
                  key={attachment.id}
                >
                  {isImagePreview ? (
                    <button
                      className="composer-attachment__image-button"
                      onClick={() =>
                        openImagePreview({
                          alt: attachment.name,
                          fileName: attachment.name,
                          src: attachment.previewUrl ?? ''
                        })
                      }
                      title={attachment.name}
                      type="button"
                    >
                      <img
                        className="composer-attachment__thumbnail"
                        src={attachment.previewUrl}
                        alt={attachment.name}
                      />
                    </button>
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

        <ComposerSelectedSkills
          catalog={skillCatalog}
          onRemove={removeSkill}
          selections={draft.skills}
        />

        <textarea
          ref={textareaRef}
          value={message}
          placeholder={t('chat.inputPlaceholder')}
          aria-label={t('chat.inputAria')}
          disabled={isModelTransitionRunning}
          rows={1}
          onChange={(event) => updateDraftMessage(event.target.value)}
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
            void submitMessage()
          }}
        />

        {hasUnsupportedImageAttachment && (
          <p className="chat-composer__warning">{t('chat.unsupportedImageWarning')}</p>
        )}
        {attachmentError && <p className="chat-composer__warning">{attachmentError}</p>}
        {hasStaleSkillSelection && (
          <p className="chat-composer__warning" role="alert">
            {t('chat.skillStaleDescription')}
          </p>
        )}
        {hasUnavailableSkillSelection && (
          <p className="chat-composer__warning" role="alert">
            {t('chat.skillUnavailableDescription')}
          </p>
        )}

        <div className="chat-composer__toolbar">
          <div className="composer-add-picker" ref={attachmentPickerRef}>
            <button
              ref={attachmentTriggerRef}
              type="button"
              className="composer-icon-button"
              aria-haspopup={isSkillMenuOpen ? 'dialog' : 'menu'}
              aria-expanded={isAttachmentMenuOpen || isSkillMenuOpen}
              aria-label={t('chat.addContext')}
              disabled={isModelTransitionRunning}
              onClick={() => {
                setAttachmentError(null)
                setIsAttachmentMenuOpen((open) => !open)
                setIsSkillMenuOpen(false)
                setIsPermissionMenuOpen(false)
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
                {!isGenerating && (
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setIsAttachmentMenuOpen(false)
                      setIsSkillMenuOpen(true)
                      setSkillSearch('')
                    }}
                  >
                    <Sparkles aria-hidden="true" />
                    <span>{t('chat.skills')}</span>
                  </button>
                )}
              </div>
            )}

            {isSkillMenuOpen && (
              <ComposerSkillPicker
                catalogState={skillCatalogState}
                projectId={selectedProjectId}
                search={skillSearch}
                selections={draft.skills}
                onClose={() => {
                  setIsSkillMenuOpen(false)
                  setSkillSearch('')
                  window.requestAnimationFrame(() => attachmentTriggerRef.current?.focus())
                }}
                onRefresh={refreshSkillCatalog}
                onSearchChange={setSkillSearch}
                onToggle={toggleSkill}
                onUseLatest={useLatestSkill}
              />
            )}
          </div>

          <div className="composer-permission-picker" ref={permissionPickerRef}>
            <button
              type="button"
              className="composer-permission-button"
              disabled={isGenerating || isModelTransitionRunning}
              data-permission={selectedPermission.id}
              aria-haspopup="listbox"
              aria-expanded={isPermissionMenuOpen}
              aria-label={`${t('chat.permission')}：${t(selectedPermission.labelKey)}`}
              onClick={() => {
                setIsAttachmentMenuOpen(false)
                setIsSkillMenuOpen(false)
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
                      onClick={() => selectPermissionMode(option.id)}
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

          {contextWindowIndicatorEnabled && contextWindowSnapshot && (
            <ContextWindowIndicator snapshot={contextWindowSnapshot} />
          )}

          <ModelConfigPicker
            ariaLabel={t('chat.selectModel')}
            disabled={isGenerating || isModelTransitionRunning}
            emptyLabel={t('chat.noEnabledModels')}
            onChange={(modelId) => {
              setIsAttachmentMenuOpen(false)
              setIsSkillMenuOpen(false)
              setIsPermissionMenuOpen(false)
              if (modelId !== selectedModel?.id) updateDraft({ modelId })
            }}
            options={enabledModels.map((model) => ({
              capabilityLabel: model.supportsImage
                ? t('configuration.image')
                : t('configuration.text'),
              capabilitySupported: model.supportsImage,
              id: model.id,
              label: model.displayName
            }))}
            value={selectedModel?.id ?? null}
            variant="composer"
          />

          <button
            type={isModelTransitionRunning || (isGenerating && !canSend) ? 'button' : 'submit'}
            className="composer-submit-button"
            data-state={submitButtonState}
            disabled={isModelTransitionRunning || (!isGenerating && !canSend)}
            aria-label={
              isGenerating ? (canSend ? t('chat.queueMessage') : t('chat.stop')) : t('chat.send')
            }
            onClick={() => {
              if (isGenerating && !canSend) {
                onStopGenerating?.()
              }
            }}
          >
            {isGenerating && !canSend ? (
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
                disabled={isModelTransitionRunning}
                aria-haspopup="listbox"
                aria-expanded={isProjectMenuOpen}
                onClick={() => {
                  setIsAttachmentMenuOpen(false)
                  setIsSkillMenuOpen(false)
                  setIsPermissionMenuOpen(false)
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
                            updateDraft({
                              projectId: project.id,
                              skills:
                                draftRef.current.projectId === project.id
                                  ? draftRef.current.skills
                                  : retainGlobalSkillSelections(draftRef.current.skills)
                            })
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
                      updateDraft({
                        projectId: null,
                        skills: retainGlobalSkillSelections(draftRef.current.skills)
                      })
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
        {isFullPermissionConfirmationOpen && (
          <ConfirmationDialog
            cancelLabel={t('chat.fullPermissionConfirmCancel')}
            confirmLabel={t('chat.fullPermissionConfirmAction')}
            description={t('chat.fullPermissionConfirmDescription')}
            onCancel={() => setIsFullPermissionConfirmationOpen(false)}
            onConfirm={() => {
              if (permissionModeAvailability.full) {
                updateDraft({ permissionMode: 'full' })
              }
              setIsFullPermissionConfirmationOpen(false)
            }}
            title={t('chat.fullPermissionConfirmTitle')}
          />
        )}
      </form>
    </div>
  )
}
