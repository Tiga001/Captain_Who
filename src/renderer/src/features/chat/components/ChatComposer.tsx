import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent
} from 'react'
import type { AgentContextWindowSnapshot, SkillDescriptor } from '@mycopilot/protocol'
import { ArrowUp, Check, ChevronDown, Folder, Plus, Search, X } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import { useLicense } from '../../license/LicenseContext'
import { useTurnAccessIdentity } from '../../license/useTurnAccessIdentity'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useProjectSettings } from '../../../config/ProjectSettingsProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { AnchoredPopover } from '../../../components/overlay/AnchoredPopover'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import {
  buildAgentInputAttachments,
  composerAttachmentFromAgentAttachment,
  getComposerDroppedFilePath,
  loadComposerAttachmentImage,
  loadComposerFoldersFromPaths,
  selectComposerFolders,
  stripAttachmentSummary
} from '../chatAttachments'
import type { ComposerAttachmentKind } from '../chatAttachments'
import { useAttachmentImports, useComposerAttachmentPreviews } from '../useAttachmentImports'
import type {
  ChatComposerDraft,
  ChatWorkspaceMention,
  ChatPermissionMode,
  ChatQueuedMessage,
  ChatSubmitOptions
} from '../chatTypes'
import { searchWorkspaceMentions } from '../../files/filesClient'
import type { WorkspaceMentionSearchEntry } from '@mycopilot/protocol'
import {
  filterSkillDescriptors,
  MAX_SELECTED_SKILLS,
  matchSkillSelection,
  removeSkillSelection,
  retainGlobalSkillSelections,
  toggleSkillSelection,
  updateSkillSelectionRevision
} from '../../skills/skillSelection'
import { useSkillCatalog } from '../../skills/useSkillCatalog'
import { getSkillPresentation, sortSkillsForDisplay } from '../../skills/skillPresentation'
import { SkillIcon } from '../../skills/SkillIcon'
import { ContextWindowIndicator } from './ContextWindowIndicator'
import { WorkspaceFileTypeIcon } from '../../../components/files/WorkspaceFileTypeIcon'
import { ComposerAttachments } from './ComposerAttachments'
import { ComposerFolderReferences } from './ComposerFolderReferences'
import { ComposerSelectedSkills } from './ComposerSelectedSkills'
import { GuidanceQueue } from './GuidanceQueue'
import { useImagePreview } from './ImagePreview'
import {
  buildMessageContentWithWorkspaceMentions,
  workspaceReferenceTargetFromMention,
  type WorkspaceReferenceTarget
} from '../workspaceMentions'
import { ModelConfigPicker } from '../../modelSelection/ModelConfigPicker'
import { createComposerModelMenuOption } from '../../modelSelection/composerModelPresentation'
import { ComposerModelMenu } from './ComposerModelMenu'
import {
  CHAT_PERMISSION_PRESENTATIONS,
  getChatPermissionPresentation
} from '../chatPermissionPresentation'
import {
  ComposerAddMenu,
  ComposerCommands,
  filterComposerCommands,
  type ComposerAddMenuSkill,
  type ComposerCommand
} from './ComposerCommands'
import './ChatComposer.css'
import './GuidanceQueue.css'

const TEXTAREA_MAX_HEIGHT = 220
const CapabilityCenterMenu = lazy(async () => ({
  default: (await import('../../capabilities/CapabilityCenterMenu')).CapabilityCenterMenu
}))

interface ChatComposerProps {
  /** Another interaction occupies the input area; preserve drafts but close transient menus. */
  isSuspended?: boolean
  commands?: readonly ComposerCommand[]
  isManualCompactionRunning?: boolean
  contextWindowIndicatorEnabled?: boolean
  contextWindowSnapshot?: AgentContextWindowSnapshot
  draft: ChatComposerDraft
  defaultProjectId?: string | null
  canGuideQueuedMessages?: boolean
  isGenerating?: boolean
  isModelTransitionRunning?: boolean
  inputPlaceholder?: string
  messageSyncKey?: string
  onDraftChange: (draft: ChatComposerDraft) => void
  onDraftMessageChange?: (draft: ChatComposerDraft) => void
  onGuideQueuedMessage?: (message: ChatQueuedMessage) => void
  queueAutoSendEnabled?: boolean
  onToggleQueueAutoSend?: () => void
  onSubmitMessage?: (
    message: string,
    options: ChatSubmitOptions
  ) => boolean | void | Promise<boolean | void>
  onStopGenerating?: () => void
  permissionModeAvailability?: {
    custom: boolean
    full: boolean
  }
  portalMenus?: boolean
  resetKey?: string
  skillCatalogRefreshToken?: number
  showProjectSelector?: boolean
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
}

const EMPTY_COMMANDS: readonly ComposerCommand[] = []

export function ChatComposer({
  isSuspended = false,
  commands = EMPTY_COMMANDS,
  isManualCompactionRunning = false,
  contextWindowIndicatorEnabled = false,
  contextWindowSnapshot,
  draft,
  defaultProjectId = null,
  canGuideQueuedMessages = false,
  isGenerating = false,
  isModelTransitionRunning = false,
  inputPlaceholder,
  messageSyncKey,
  onDraftChange,
  onDraftMessageChange,
  onGuideQueuedMessage,
  queueAutoSendEnabled = false,
  onToggleQueueAutoSend,
  onSubmitMessage,
  onStopGenerating,
  permissionModeAvailability = { custom: true, full: true },
  portalMenus = false,
  resetKey,
  skillCatalogRefreshToken = 0,
  showProjectSelector = false,
  onOpenWorkspaceReference
}: ChatComposerProps) {
  const { t } = useFrontendConfig()
  const accountAuth = useAccountAuth()
  const license = useLicense()
  const accessIdentity = useTurnAccessIdentity()
  const submitInFlightRef = useRef(false)
  const { enabledModels } = useModelSettings()
  const { projects, openCreateProjectDialog } = useProjectSettings()
  const openImagePreview = useImagePreview()
  const commandListId = useId()
  const [isCommandMenuOpen, setIsCommandMenuOpen] = useState(false)
  const [isCapabilityCenterOpen, setIsCapabilityCenterOpen] = useState(false)
  const [isModelMenuOpen, setIsModelMenuOpen] = useState(false)
  const [isCapabilityDialogOpen, setIsCapabilityDialogOpen] = useState(false)
  const [isCommandSession, setIsCommandSession] = useState(false)
  const [commandIndex, setCommandIndex] = useState(0)
  const commandTriggerRef = useRef(false)
  const commandExecutingRef = useRef(false)
  const commandMenuRef = useRef<HTMLDivElement>(null)
  const commandPopoverRef = useRef<HTMLDivElement>(null)
  const draftRef = useRef(draft)
  const composerRef = useRef<HTMLFormElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const isComposingRef = useRef(false)
  const lastCompositionEndAtRef = useRef(0)
  const attachmentPickerRef = useRef<HTMLDivElement>(null)
  const attachmentPreviewRequestRef = useRef(0)
  const attachmentTriggerRef = useRef<HTMLButtonElement>(null)
  const attachmentPopoverRef = useRef<HTMLDivElement>(null)
  const permissionPickerRef = useRef<HTMLDivElement>(null)
  const permissionTriggerRef = useRef<HTMLButtonElement>(null)
  const permissionPopoverRef = useRef<HTMLDivElement>(null)
  const projectPickerRef = useRef<HTMLDivElement>(null)
  const projectTriggerRef = useRef<HTMLButtonElement>(null)
  const projectPopoverRef = useRef<HTMLDivElement>(null)
  const [isAttachmentMenuOpen, setIsAttachmentMenuOpen] = useState(false)
  const [isPermissionMenuOpen, setIsPermissionMenuOpen] = useState(false)
  const [isFullPermissionConfirmationOpen, setIsFullPermissionConfirmationOpen] = useState(false)
  const [isProjectMenuOpen, setIsProjectMenuOpen] = useState(false)
  const [isMentionMenuOpen, setIsMentionMenuOpen] = useState(false)
  const [mentionQuery, setMentionQuery] = useState('')
  const [mentionEntries, setMentionEntries] = useState<WorkspaceMentionSearchEntry[]>([])
  const [mentionIndex, setMentionIndex] = useState(0)
  const [addMenuIndex, setAddMenuIndex] = useState(0)
  const mentionRequestRef = useRef(0)
  const mentionPopoverRef = useRef<HTMLDivElement>(null)
  const [isFileDragActive, setIsFileDragActive] = useState(false)
  const [attachmentError, setAttachmentError] = useState<string | null>(null)
  const [projectSearch, setProjectSearch] = useState('')
  const [message, setMessage] = useState(draft.message)
  const isCommandSubmenuOpen = isCapabilityCenterOpen || isModelMenuOpen
  const filteredCommands = isCommandSession ? filterComposerCommands(commands, message) : []
  const selectedCommandIndex = Math.min(commandIndex, Math.max(0, filteredCommands.length - 1))
  const selectedCommand = filteredCommands[selectedCommandIndex]
  const hasCommandSelection = isCommandSession && filteredCommands.length > 0
  const previousSkillScopeRef = useRef({ projectId: draft.projectId, resetKey })
  const previousResetKeyRef = useRef(resetKey)
  const previousMessageSyncKeyRef = useRef(messageSyncKey)
  const lastExternalMessageRef = useRef(draft.message)
  const permissionOptions = useMemo(
    () =>
      CHAT_PERMISSION_PRESENTATIONS.filter(
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
  const updateDraft = useCallback(
    (patch: Partial<ChatComposerDraft>) => {
      const currentDraft = draftRef.current
      const nextDraft = {
        ...currentDraft,
        ...patch,
        updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
      }
      if (patch.message !== undefined) {
        setMessage(patch.message)
      }
      draftRef.current = nextDraft
      onDraftChange({
        ...nextDraft
      })
    },
    [onDraftChange]
  )

  const storedAttachments = useMemo(
    () => draft.attachments.map(composerAttachmentFromAgentAttachment),
    [draft.attachments]
  )
  const attachments = useComposerAttachmentPreviews(storedAttachments)
  const attachmentImports = useAttachmentImports({
    scope: JSON.stringify([resetKey, draft.projectId]),
    existingAttachments: attachments,
    onAttachments: (nextAttachments) => {
      const currentAttachments = draftRef.current.attachments
      const existing = new Set(currentAttachments.map((attachment) => attachment.id))
      updateDraft({
        attachments: [
          ...currentAttachments,
          ...buildAgentInputAttachments(nextAttachments).filter(
            (attachment) => !existing.has(attachment.id)
          )
        ]
      })
      setAttachmentError(null)
    },
    onError: (error) =>
      setAttachmentError(getUserFacingErrorMessage(error, t, 'chat.attachmentOperationFailed')),
    errorMessage: (error) => getUserFacingErrorMessage(error, t, 'chat.attachmentOperationFailed')
  })
  useLayoutEffect(() => {
    attachmentPreviewRequestRef.current += 1
    return () => {
      attachmentPreviewRequestRef.current += 1
    }
  }, [resetKey, draft.projectId])
  const selectedPermission =
    permissionOptions.find((option) => option.id === permissionMode) ??
    getChatPermissionPresentation('default')
  const SelectedPermissionIcon = selectedPermission.icon
  const selectedModel = useMemo(() => {
    return enabledModels.find((model) => model.id === selectedModelId) ?? enabledModels[0]
  }, [enabledModels, selectedModelId])
  const modelOptions = useMemo(
    () => enabledModels.map((model) => createComposerModelMenuOption(model, t)),
    [enabledModels, t]
  )
  const isModelSelectionDisabled = isModelTransitionRunning || isManualCompactionRunning
  const nextTurnConfigurationHint = isGenerating ? t('chat.nextTurnConfigurationHint') : undefined
  const selectedProject = projects.find((project) => project.id === selectedProjectId)
  const skillCatalogEnabled =
    isAttachmentMenuOpen ||
    (isMentionMenuOpen && mentionQuery.trim() === '') ||
    draft.skills.length > 0
  const skillCatalogRefreshKey = `${skillCatalogRefreshToken}\u0002${
    isAttachmentMenuOpen || (isMentionMenuOpen && mentionQuery.trim() === '')
      ? 'composer-menu-open'
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
  const addMenuSkills = useMemo<ComposerAddMenuSkill[]>(() => {
    if (!skillCatalog || isGenerating) return []
    const selectedIds = new Set(draft.skills.map((selection) => selection.id))
    return sortSkillsForDisplay(skillCatalogDescriptors).map((skill) => {
      const presentation = getSkillPresentation(skill, t)
      const selected = selectedIds.has(skill.id)
      const sourceLabel =
        skill.source.kind === 'workspace'
          ? t('chat.workspaceSkill')
          : skill.source.kind === 'bundled'
            ? t('chat.bundledSkill')
            : t('chat.installedSkill')
      const trustLabel =
        skill.trust === 'untrusted'
          ? t('chat.skillTrustUntrusted')
          : t('chat.skillTrustApplication')
      return {
        id: skill.id,
        label: presentation.name,
        accessibleLabel: `${presentation.name} · ${sourceLabel} · ${trustLabel}`,
        description: presentation.description,
        icon: <SkillIcon skillId={skill.id} source={skill.source} />,
        selected,
        disabled: !selected && draft.skills.length >= MAX_SELECTED_SKILLS
      }
    })
  }, [draft.skills, isGenerating, skillCatalog, skillCatalogDescriptors, t])
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
  const folderReferences = draft.folderReferences ?? []
  const workspaceMentions = draft.workspaceMentions ?? []
  const hasSendableContent =
    message.trim().length > 0 ||
    attachments.length > 0 ||
    folderReferences.length > 0 ||
    workspaceMentions.length > 0
  const hasInvalidSkillSelection = hasStaleSkillSelection || hasUnavailableSkillSelection
  const canSend =
    hasSendableContent &&
    !hasUnsupportedImageAttachment &&
    !hasInvalidSkillSelection &&
    !attachmentImports.hasPending &&
    !isModelTransitionRunning &&
    !isManualCompactionRunning &&
    Boolean(selectedModel)
  const submitButtonState = isCommandSubmenuOpen
    ? 'disabled'
    : hasCommandSelection
      ? selectedCommand?.disabledReason
        ? 'disabled'
        : 'ready'
      : isModelTransitionRunning || isManualCompactionRunning
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
    commandMenuRef,
    isCommandMenuOpen && !isCapabilityDialogOpen,
    () => setIsCommandMenuOpen(false),
    (target) =>
      Boolean(textareaRef.current?.contains(target) || commandPopoverRef.current?.contains(target))
  )
  useEffect(() => {
    if (!isCommandMenuOpen) {
      setIsCapabilityCenterOpen(false)
      setIsModelMenuOpen(false)
      setIsCapabilityDialogOpen(false)
    }
  }, [isCommandMenuOpen])
  useEffect(() => {
    if (isAttachmentMenuOpen || isPermissionMenuOpen || isProjectMenuOpen)
      setIsCommandMenuOpen(false)
  }, [isAttachmentMenuOpen, isPermissionMenuOpen, isProjectMenuOpen])

  useDismissOnOutsidePointer(
    attachmentPickerRef,
    isAttachmentMenuOpen,
    useCallback(() => {
      setIsAttachmentMenuOpen(false)
    }, []),
    (target) =>
      Boolean(
        attachmentPopoverRef.current?.contains(target) || composerRef.current?.contains(target)
      )
  )
  useDismissOnOutsidePointer(
    permissionPickerRef,
    isPermissionMenuOpen,
    () => setIsPermissionMenuOpen(false),
    (target) => Boolean(permissionPopoverRef.current?.contains(target))
  )
  useDismissOnOutsidePointer(
    projectPickerRef,
    isProjectMenuOpen,
    () => setIsProjectMenuOpen(false),
    (target) => Boolean(projectPopoverRef.current?.contains(target))
  )

  useLayoutEffect(() => {
    if (previousResetKeyRef.current !== resetKey) {
      previousResetKeyRef.current = resetKey
      previousMessageSyncKeyRef.current = messageSyncKey
      lastExternalMessageRef.current = draft.message
      setIsCommandSession(false)
      setIsCommandMenuOpen(false)
      commandTriggerRef.current = false
      setMessage(draft.message)
      draftRef.current = draft
      return
    }

    // Keystrokes update the local snapshot and the owner's ref without refreshing `draft`.
    // Keep that whole newer version: combining its text with the older prop timestamp makes
    // submit mistake the already-sent input for a different draft and refuse to consume it.
    if (draft.updatedAt < draftRef.current.updatedAt) {
      previousMessageSyncKeyRef.current = messageSyncKey
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
      setIsCommandSession(false)
      setIsCommandMenuOpen(false)
      commandTriggerRef.current = false
      setMessage(draft.message)
      draftRef.current = draft
      return
    }

    if (draft.message !== lastExternalMessageRef.current) {
      lastExternalMessageRef.current = draft.message
      // A parent may mirror keystrokes synchronously. Only a different external value restores a draft.
      if (draft.message !== message) {
        setIsCommandSession(false)
        setIsCommandMenuOpen(false)
        commandTriggerRef.current = false
      }
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
    setIsPermissionMenuOpen(false)
    setIsProjectMenuOpen(false)
    setIsMentionMenuOpen(false)
    setMentionQuery('')
  }, [isModelTransitionRunning])

  useEffect(() => {
    if (!isSuspended) return
    setIsCommandMenuOpen(false)
    setIsAttachmentMenuOpen(false)
    setIsPermissionMenuOpen(false)
    setIsProjectMenuOpen(false)
    setIsMentionMenuOpen(false)
    setIsFullPermissionConfirmationOpen(false)
  }, [isSuspended])

  // The Host owns filesystem access. Keep the renderer query-only and discard stale responses.
  useEffect(() => {
    if (!isMentionMenuOpen || !mentionQuery.trim() || !selectedProjectId) {
      mentionRequestRef.current += 1
      setMentionEntries([])
      return
    }
    const requestId = ++mentionRequestRef.current
    const timer = window.setTimeout(() => {
      void searchWorkspaceMentions({ projectId: selectedProjectId, query: mentionQuery, limit: 24 })
        .then((result) => {
          if (mentionRequestRef.current !== requestId) return
          setMentionEntries(result.entries)
          setMentionIndex(0)
        })
        .catch(() => {
          if (mentionRequestRef.current === requestId) setMentionEntries([])
        })
    }, 80)
    return () => window.clearTimeout(timer)
  }, [isMentionMenuOpen, mentionQuery, selectedProjectId])

  useEffect(() => {
    setAttachmentError(null)
    setIsAttachmentMenuOpen(false)
    setIsFullPermissionConfirmationOpen(false)
    setIsFileDragActive(false)
    setIsMentionMenuOpen(false)
    setMentionQuery('')
  }, [resetKey])

  const updateDraftMessage = useCallback(
    (nextMessage: string) => {
      const nextDraft = {
        ...draftRef.current,
        message: nextMessage,
        updatedAt: Math.max(Date.now(), draftRef.current.updatedAt + 1)
      }
      setMessage(nextMessage)
      draftRef.current = nextDraft
      onDraftMessageChange?.({ ...nextDraft })
    },
    [onDraftMessageChange]
  )

  const selectWorkspaceMention = useCallback(
    (entry: WorkspaceMentionSearchEntry) => {
      if (!selectedProjectId) return
      const mention: ChatWorkspaceMention = {
        id: `${entry.folderId}:${entry.path}`,
        projectId: selectedProjectId,
        folderId: entry.folderId,
        alias: entry.alias,
        displayName: entry.displayName,
        path: entry.path,
        displayPath: entry.displayPath,
        kind: entry.kind === 'directory' ? 'directory' : 'file'
      }
      const existing = draftRef.current.workspaceMentions ?? []
      if (!existing.some((item) => item.id === mention.id)) {
        updateDraft({ workspaceMentions: [...existing, mention], message: '' })
      } else {
        updateDraft({ message: '' })
      }
      setMentionQuery('')
      setMentionEntries([])
      setMentionIndex(0)
      setIsMentionMenuOpen(false)
      window.requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }))
    },
    [selectedProjectId, updateDraft]
  )

  const removeWorkspaceMention = (id: string) => {
    updateDraft({
      workspaceMentions: (draftRef.current.workspaceMentions ?? []).filter(
        (mention) => mention.id !== id
      )
    })
  }

  const clearSelectedProject = useCallback(() => {
    updateDraft({
      projectId: null,
      skills: retainGlobalSkillSelections(draftRef.current.skills)
    })
    setProjectSearch('')
    setIsProjectMenuOpen(false)
  }, [updateDraft])

  // Both model menus use this selection path. Only the slash menu also consumes its local query.
  const selectModelConfig = (modelId: string) => {
    if (isModelSelectionDisabled || !enabledModels.some((model) => model.id === modelId)) return
    setIsAttachmentMenuOpen(false)
    setIsPermissionMenuOpen(false)
    if (isModelMenuOpen) {
      setIsCommandMenuOpen(false)
      setIsCommandSession(false)
      updateDraft({ modelId, message: '' })
      textareaRef.current?.focus({ preventScroll: true })
    } else if (modelId !== selectedModel?.id) {
      updateDraft({ modelId })
    }
  }

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

  useLayoutEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    const nextHeight = Math.min(textarea.scrollHeight, TEXTAREA_MAX_HEIGHT)
    textarea.style.height = `${nextHeight}px`
    textarea.style.overflowY = textarea.scrollHeight > TEXTAREA_MAX_HEIGHT ? 'auto' : 'hidden'
  }, [message])

  const executeCommand = async (command: ComposerCommand) => {
    if (command.disabledReason || commandExecutingRef.current) return
    if (command.id === 'capabilities') {
      setIsModelMenuOpen(false)
      setIsCapabilityCenterOpen(true)
      setIsCommandMenuOpen(true)
      return
    }
    if (command.id === 'model') {
      setIsCapabilityCenterOpen(false)
      setIsModelMenuOpen(true)
      setIsCommandMenuOpen(true)
      return
    }
    commandExecutingRef.current = true
    setIsCommandMenuOpen(false)
    setIsCommandSession(false)
    updateDraft({ message: '' })
    try {
      await command.execute()
    } catch (error) {
      setAttachmentError(getUserFacingErrorMessage(error, t, 'chat.commands.failed'))
    } finally {
      commandExecutingRef.current = false
    }
  }

  const submitMessage = async () => {
    if (submitInFlightRef.current) return
    if (
      isComposingRef.current ||
      Date.now() - lastCompositionEndAtRef.current < 120 ||
      commandExecutingRef.current ||
      isCommandSubmenuOpen
    )
      return
    if (hasCommandSelection && selectedCommand) {
      await executeCommand(selectedCommand)
      return
    }
    if (!canSend) return
    const submissionIdentity = accessIdentity.current
    if (!isGenerating && accountAuth && !accountAuth.canStartTurn()) {
      accountAuth.requestLogin()
      return
    }
    if (!isGenerating && license && !license.canStartTurn()) {
      license.requestAccess()
      return
    }

    setIsCommandMenuOpen(false)
    setIsCommandSession(false)
    const submittedDraft = draftRef.current
    const trimmedMessage = message.trim()
    const messageContent = buildMessageContentWithWorkspaceMentions(
      trimmedMessage,
      submittedDraft.workspaceMentions ?? []
    )
    let inputAttachments: ChatSubmitOptions['attachments']

    try {
      inputAttachments = await buildAgentInputAttachments(attachments)
      setAttachmentError(null)
    } catch (error) {
      setAttachmentError(getUserFacingErrorMessage(error, t, 'chat.attachmentOperationFailed'))
      return
    }

    if (previousResetKeyRef.current !== resetKey) return
    if (!isGenerating && submissionIdentity !== accessIdentity.current) return

    const submitOptions: ChatSubmitOptions = {
      draftSnapshot: submittedDraft,
      attachments: inputAttachments,
      folderReferences: submittedDraft.folderReferences ?? [],
      workspaceMentions: submittedDraft.workspaceMentions ?? [],
      modelId: selectedModel?.id ?? selectedModelId,
      permissionMode,
      projectId: selectedProject?.id ?? null,
      skills: [...submittedDraft.skills]
    }

    if (isGenerating) {
      const createdAt = Date.now()
      const currentDraft = draftRef.current
      updateDraft({
        message: currentDraft.message === submittedDraft.message ? '' : currentDraft.message,
        attachments:
          currentDraft.attachments === submittedDraft.attachments ? [] : currentDraft.attachments,
        folderReferences:
          currentDraft.folderReferences === submittedDraft.folderReferences
            ? []
            : currentDraft.folderReferences,
        workspaceMentions:
          currentDraft.workspaceMentions === submittedDraft.workspaceMentions
            ? []
            : currentDraft.workspaceMentions,
        queuedMessages: [
          ...draftRef.current.queuedMessages,
          {
            id: `queued-message-${createdAt}-${Math.random().toString(36).slice(2, 8)}`,
            clientMessageId: `guidance-${createdAt}-${Math.random().toString(36).slice(2, 10)}`,
            content: messageContent,
            attachments: inputAttachments ?? [],
            folderReferences: submittedDraft.folderReferences ?? [],
            workspaceMentions: submittedDraft.workspaceMentions ?? [],
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
      return
    }

    if (submitInFlightRef.current) return
    submitInFlightRef.current = true
    let accepted: boolean | void
    try {
      accepted = await onSubmitMessage?.(messageContent, submitOptions)
    } finally {
      submitInFlightRef.current = false
    }
    if (accepted === false || previousResetKeyRef.current !== resetKey) return
    // The submission owner consumes/restores the draft together with the optimistic message.
    // A late acceptance must never clear the next draft, even if its text is identical.
    setIsAttachmentMenuOpen(false)
    setIsPermissionMenuOpen(false)
    setIsProjectMenuOpen(false)
  }

  const returnToCommands = () => {
    setIsCapabilityCenterOpen(false)
    setIsModelMenuOpen(false)
    textareaRef.current?.focus({ preventScroll: true })
  }

  const removeAttachment = (attachmentId: string) => {
    updateDraft({
      attachments: draftRef.current.attachments.filter(
        (attachment) => attachment.id !== attachmentId
      )
    })
  }

  const removeFolderReference = (folderId: string) => {
    updateDraft({
      folderReferences: (draftRef.current.folderReferences ?? []).filter(
        (folder) => folder.id !== folderId
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

    attachmentImports.cancelAll()
    updateDraft({
      message: stripAttachmentSummary(queuedMessage.content, queuedMessage.attachments),
      attachments: queuedMessage.attachments,
      folderReferences: queuedMessage.folderReferences ?? [],
      workspaceMentions: queuedMessage.workspaceMentions ?? [],
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
    if (isModelSelectionDisabled) return
    setIsPermissionMenuOpen(false)
    if (nextPermissionMode === permissionMode) return

    if (nextPermissionMode === 'full') {
      setIsFullPermissionConfirmationOpen(true)
      return
    }

    updateDraft({ permissionMode: nextPermissionMode })
  }

  const toggleSkill = (skill: SkillDescriptor) => {
    const selection = draftRef.current.skills.find((candidate) => candidate.id === skill.id)
    if (selection && selection.revision !== skill.revision) {
      updateDraft({
        skills: updateSkillSelectionRevision(draftRef.current.skills, skill)
      })
      return
    }
    updateDraft({
      skills: toggleSkillSelection(draftRef.current.skills, skill)
    })
  }

  const addAttachments = async (kind: ComposerAttachmentKind) => {
    setAttachmentError(null)
    if (message === '@') updateDraft({ message: '' })
    setMentionQuery('')
    setIsMentionMenuOpen(false)
    setIsAttachmentMenuOpen(false)
    await attachmentImports.select(kind)
  }

  const addFolders = async () => {
    setAttachmentError(null)
    if (message === '@') updateDraft({ message: '' })
    setMentionQuery('')
    setIsMentionMenuOpen(false)
    setIsAttachmentMenuOpen(false)
    try {
      const selected = await selectComposerFolders()
      const existing = new Set(
        (draftRef.current.folderReferences ?? []).map((folder) => folder.rootPath ?? folder.id)
      )
      const additions = selected.filter((folder) => !existing.has(folder.rootPath ?? folder.id))
      if (additions.length > 0) {
        updateDraft({
          folderReferences: [...(draftRef.current.folderReferences ?? []), ...additions]
        })
      }
    } catch (error) {
      setAttachmentError(getUserFacingErrorMessage(error, t, 'chat.attachmentOperationFailed'))
    }
  }

  const mentionHomeItemCount = 3 + (!isGenerating ? addMenuSkills.length : 0)
  const addMenuItemCount = 3 + (!isGenerating ? addMenuSkills.length : 0)
  const activateAddMenuItem = (index: number) => {
    if (index === 0) {
      void addAttachments('file')
      return
    }
    if (index === 1) {
      void addFolders()
      return
    }
    if (index === 2) {
      void addAttachments('image')
      return
    }
    const skillId = addMenuSkills[index - 3]?.id
    const skill = skillId
      ? skillCatalogDescriptors.find((descriptor) => descriptor.id === skillId)
      : undefined
    if (skill && !isGenerating) toggleSkill(skill)
  }

  const handleAddMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!isAttachmentMenuOpen || event.defaultPrevented) return
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      setAddMenuIndex((current) => {
        const count = Math.max(1, addMenuItemCount)
        return (current + (event.key === 'ArrowDown' ? 1 : count - 1)) % count
      })
      return
    }
    if ((event.key === 'Enter' || event.key === 'Tab') && addMenuIndex < addMenuItemCount) {
      event.preventDefault()
      activateAddMenuItem(addMenuIndex)
    }
  }
  const activateMentionHomeItem = (index: number) => {
    if (index === 0) {
      void addAttachments('file')
      return
    }
    if (index === 1) {
      void addFolders()
      return
    }
    if (index === 2) {
      void addAttachments('image')
      return
    }
    const skillId = addMenuSkills[index - 3]?.id
    const skill = skillId
      ? skillCatalogDescriptors.find((descriptor) => descriptor.id === skillId)
      : undefined
    if (!skill || isGenerating) return
    if (message === '@') updateDraft({ message: '' })
    toggleSkill(skill)
    setMentionQuery('')
    setMentionEntries([])
  }

  const addDroppedOrPastedFiles = async (files: FileList | File[]) => {
    if (files.length === 0) return
    setAttachmentError(null)
    await attachmentImports.addFiles(files)
  }

  const addDroppedItems = async (dataTransfer: DataTransfer) => {
    const folderPaths = Array.from(dataTransfer.items)
      .filter((item) => item.kind === 'file')
      .map((item) => {
        const file = item.getAsFile()
        const entry = (
          item as DataTransferItem & { webkitGetAsEntry?: () => FileSystemEntry | null }
        ).webkitGetAsEntry?.()
        return entry?.isDirectory && file ? getComposerDroppedFilePath(file) : undefined
      })
      .filter((path): path is string => Boolean(path))
      .filter((path, index, paths) => paths.indexOf(path) === index)
    if (folderPaths.length > 0) {
      try {
        const selected = await loadComposerFoldersFromPaths(folderPaths)
        const existing = new Set(
          (draftRef.current.folderReferences ?? []).map((folder) => folder.rootPath ?? folder.id)
        )
        const additions = selected.filter((folder) => !existing.has(folder.rootPath ?? folder.id))
        if (additions.length > 0) {
          updateDraft({
            folderReferences: [...(draftRef.current.folderReferences ?? []), ...additions]
          })
        }
      } catch (error) {
        setAttachmentError(getUserFacingErrorMessage(error, t, 'chat.attachmentOperationFailed'))
      }
    }
    const files = Array.from(dataTransfer.files).filter((file) => {
      const path = getComposerDroppedFilePath(file)
      return !path || !folderPaths.includes(path)
    })
    if (files.length > 0) await addDroppedOrPastedFiles(files)
  }

  const handleSelectProjectDirectory = async () => {
    setIsProjectMenuOpen(false)
    const project = await openCreateProjectDialog()
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
      <div
        ref={commandMenuRef}
        onKeyDown={(event) => {
          if (
            !isCommandSubmenuOpen ||
            isCapabilityDialogOpen ||
            event.defaultPrevented ||
            event.key !== 'Escape'
          )
            return
          if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) {
            event.stopPropagation()
            return
          }
          event.preventDefault()
          event.stopPropagation()
          returnToCommands()
        }}
      >
        {isCommandMenuOpen && (
          <AnchoredPopover
            anchorRef={composerRef}
            enabled={portalMenus}
            matchAnchorWidth
            placement="top"
            returnFocusRef={textareaRef}
            onClose={() => {
              if (!isCapabilityDialogOpen) setIsCommandMenuOpen(false)
            }}
            popoverRef={commandPopoverRef}
          >
            {isModelMenuOpen ? (
              <ComposerModelMenu
                disabled={isModelSelectionDisabled}
                options={modelOptions}
                value={selectedModel?.id ?? null}
                onChange={selectModelConfig}
                onBack={returnToCommands}
                scrollContainerRef={portalMenus ? commandPopoverRef : undefined}
              />
            ) : isCapabilityCenterOpen ? (
              <Suspense fallback={null}>
                <CapabilityCenterMenu
                  onBack={returnToCommands}
                  onDialogOpenChange={setIsCapabilityDialogOpen}
                />
              </Suspense>
            ) : (
              <ComposerCommands
                commands={filteredCommands}
                query={message.slice(1)}
                selectedIndex={selectedCommandIndex}
                onSelect={setCommandIndex}
                onExecute={(command) => {
                  void executeCommand(command)
                }}
                emptyLabel={t('chat.commands.noMatch')}
                listId={commandListId}
                scrollContainerRef={portalMenus ? commandPopoverRef : undefined}
              />
            )}
          </AnchoredPopover>
        )}
        {isMentionMenuOpen && (
          <AnchoredPopover
            anchorRef={composerRef}
            className="chat-composer-menu-popover"
            enabled={portalMenus}
            matchAnchorWidth
            placement="top"
            returnFocusRef={textareaRef}
            onClose={() => setIsMentionMenuOpen(false)}
            popoverRef={mentionPopoverRef}
          >
            {mentionQuery.trim() === '' ? (
              <ComposerAddMenu
                addFileLabel={t('chat.addFile')}
                addFolderLabel={t('chat.addFolder')}
                addImageLabel={t('chat.addImage')}
                addMenuTitle={t('chat.addMenuTitle')}
                onAddFile={() => addAttachments('file')}
                onAddFolder={addFolders}
                onAddImage={() => addAttachments('image')}
                selectedIndex={mentionIndex}
                onSelectIndex={setMentionIndex}
                skills={!isGenerating ? addMenuSkills : undefined}
                skillsTitle={t('chat.skills')}
                skillLoading={skillCatalogState.status === 'loading'}
                skillLoadingLabel={t('chat.loadingSkills')}
                skillError={skillCatalogState.status === 'error'}
                skillErrorLabel={t('chat.skillsLoadFailed')}
                skillRetryLabel={t('chat.retrySkills')}
                skillCatalogTruncated={Boolean(skillCatalog?.truncated)}
                skillTruncatedLabel={t('chat.skillCatalogTruncated')}
                skillDiagnosticsCount={skillCatalog?.diagnostics.length ?? 0}
                skillDiagnosticsLabel={t('chat.skillDiagnostics')}
                skillDiagnosticsAvailableLabel={t('skills.diagnosticsAvailable')}
                skillEmptyLabel={t('chat.noSkills')}
                onRetrySkills={refreshSkillCatalog}
                onToggleSkill={(skill) => {
                  const descriptor = skillCatalogDescriptors.find((item) => item.id === skill.id)
                  if (!descriptor) return
                  if (message === '@') updateDraft({ message: '' })
                  toggleSkill(descriptor)
                }}
              />
            ) : mentionEntries.length === 0 ? (
              <div className="composer-commands" role="listbox" aria-label="Workspace files">
                <div className="composer-commands__list">
                  <p className="composer-commands__empty">未找到匹配项</p>
                </div>
              </div>
            ) : (
              <div className="composer-commands" role="listbox" aria-label="Workspace files">
                <div className="composer-commands__list">
                  {mentionEntries.map((entry, index) => (
                    <button
                      key={`${entry.folderId}:${entry.path}`}
                      type="button"
                      role="option"
                      aria-selected={index === mentionIndex}
                      onMouseDown={(event) => event.preventDefault()}
                      onPointerMove={() => setMentionIndex(index)}
                      onClick={() => selectWorkspaceMention(entry)}
                    >
                      {entry.kind === 'directory' ? (
                        <Folder aria-hidden="true" />
                      ) : (
                        <WorkspaceFileTypeIcon path={entry.path} />
                      )}
                      <span className="composer-commands__label">{entry.displayName}</span>
                      <span className="composer-commands__description" title={entry.displayPath}>
                        {entry.displayPath}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </AnchoredPopover>
        )}
      </div>
      <GuidanceQueue
        guideEnabled={canGuideQueuedMessages}
        messages={draft.queuedMessages}
        onDelete={deleteQueuedMessage}
        onEdit={editQueuedMessage}
        onGuide={(queuedMessage) => onGuideQueuedMessage?.(queuedMessage)}
        onMove={moveQueuedMessage}
        queueAutoSendEnabled={queueAutoSendEnabled}
        onToggleQueueAutoSend={onToggleQueueAutoSend}
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
          if (event.dataTransfer.files.length === 0 && event.dataTransfer.items.length === 0) return
          event.preventDefault()
          setIsFileDragActive(false)
          void addDroppedItems(event.dataTransfer)
        }}
        onPaste={(event) => {
          commandTriggerRef.current = false
          setIsCommandSession(false)
          setIsCommandMenuOpen(false)
          if (isModelTransitionRunning) return
          if (event.clipboardData.files.length === 0) return
          event.preventDefault()
          void addDroppedOrPastedFiles(event.clipboardData.files)
        }}
      >
        {(folderReferences.length > 0 ||
          attachments.length > 0 ||
          attachmentImports.pending.length > 0) && (
          <>
            <ComposerFolderReferences
              folders={folderReferences}
              label={t('chat.attachments')}
              removeLabel={t('chat.removeAttachment')}
              onRemove={removeFolderReference}
            />
            <ComposerAttachments
              attachments={[...attachments, ...attachmentImports.pending]}
              label={t('chat.attachments')}
              removeLabel={t('chat.removeAttachment')}
              cancelLabel={t('project.cancel')}
              retryLabel={t('files.retry')}
              failedLabel={t('chat.attachmentOperationFailed')}
              onRetry={(id) => void attachmentImports.retry(id)}
              onRemove={(id) => {
                if (attachmentImports.pending.some((attachment) => attachment.id === id))
                  attachmentImports.cancel(id)
                else removeAttachment(id)
              }}
              onPreview={openImagePreview}
              onPreviewAttachment={(id) => {
                const attachment = attachments.find((item) => item.id === id)
                if (!attachment) return
                const requestId = ++attachmentPreviewRequestRef.current
                const scope = resetKey
                const projectId = draft.projectId
                void loadComposerAttachmentImage(attachment.agentAttachment)
                  .then((src) => {
                    if (
                      attachmentPreviewRequestRef.current !== requestId ||
                      previousResetKeyRef.current !== scope ||
                      draftRef.current.projectId !== projectId
                    )
                      return
                    const imageSource = src ?? attachment.previewUrl
                    if (imageSource)
                      openImagePreview({
                        alt: attachment.name,
                        fileName: attachment.name,
                        src: imageSource
                      })
                  })
                  .catch(() => {
                    if (
                      attachmentPreviewRequestRef.current === requestId &&
                      previousResetKeyRef.current === scope &&
                      draftRef.current.projectId === projectId &&
                      attachment.previewUrl
                    ) {
                      openImagePreview({
                        alt: attachment.name,
                        fileName: attachment.name,
                        src: attachment.previewUrl
                      })
                    }
                  })
              }}
            />
          </>
        )}

        {(draft.skills.length > 0 || workspaceMentions.length > 0) && (
          <div className="chat-composer__context-items">
            <ComposerSelectedSkills
              catalog={skillCatalog}
              onRemove={removeSkill}
              selections={draft.skills}
            />

            {workspaceMentions.length > 0 && (
              <div className="composer-workspace-mentions" aria-label="Workspace references">
                {workspaceMentions.map((mention) => (
                  <div className="composer-workspace-mention" key={mention.id}>
                    <button
                      type="button"
                      className="composer-workspace-mention__link"
                      title={`${mention.alias}/${mention.path}`}
                      onClick={() =>
                        onOpenWorkspaceReference?.(workspaceReferenceTargetFromMention(mention))
                      }
                    >
                      {mention.kind === 'directory' ? (
                        <Folder aria-hidden="true" />
                      ) : (
                        <WorkspaceFileTypeIcon path={mention.path} />
                      )}
                      <span>{mention.displayName}</span>
                    </button>
                    <button
                      type="button"
                      className="composer-workspace-mention__remove"
                      aria-label={`Remove ${mention.displayName}`}
                      title={`Remove ${mention.displayName}`}
                      onClick={() => removeWorkspaceMention(mention.id)}
                    >
                      <X aria-hidden="true" />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        <textarea
          ref={textareaRef}
          value={message}
          placeholder={inputPlaceholder ?? t('chat.inputPlaceholder')}
          aria-label={t('chat.inputAria')}
          disabled={isModelTransitionRunning}
          rows={1}
          aria-controls={isCommandMenuOpen && !isCommandSubmenuOpen ? commandListId : undefined}
          aria-activedescendant={
            isCommandMenuOpen && !isCommandSubmenuOpen && selectedCommand
              ? `${commandListId}-${selectedCommandIndex}`
              : undefined
          }
          aria-autocomplete="list"
          onChange={(event) => {
            setIsCapabilityCenterOpen(false)
            setIsModelMenuOpen(false)
            const nextMessage = event.target.value
            const native = event.nativeEvent as InputEvent
            const onlyMentionsBeforeInput =
              (draftRef.current.workspaceMentions?.length ?? 0) > 0 && message.trim() === ''
            const typedAt = native.inputType === 'insertText' && native.data === '@'
            if (
              (typedAt && (message === '' || onlyMentionsBeforeInput)) ||
              ((message === '' || onlyMentionsBeforeInput) && nextMessage.startsWith('@'))
            ) {
              setIsMentionMenuOpen(true)
              setMentionQuery(nextMessage.slice(1))
              setMentionIndex(0)
              setIsAttachmentMenuOpen(false)
              setIsPermissionMenuOpen(false)
              setIsProjectMenuOpen(false)
              setIsCommandMenuOpen(false)
            } else if (
              nextMessage.startsWith('@') &&
              (isMentionMenuOpen || message.startsWith('@') || onlyMentionsBeforeInput)
            ) {
              setIsMentionMenuOpen(true)
              setMentionQuery(nextMessage.slice(1))
            } else if (!nextMessage.startsWith('@')) {
              setIsMentionMenuOpen(false)
              setMentionQuery('')
            }
            const typedSlash =
              commandTriggerRef.current ||
              (native.inputType === 'insertText' && native.data === '/')
            if (
              commands.length > 0 &&
              message === '' &&
              nextMessage === '/' &&
              typedSlash &&
              !isComposingRef.current &&
              !native.isComposing
            ) {
              setIsCommandSession(true)
              setIsCommandMenuOpen(true)
              setIsAttachmentMenuOpen(false)
              setIsPermissionMenuOpen(false)
              setIsProjectMenuOpen(false)
            } else if (!nextMessage.startsWith('/') || nextMessage.includes('\n')) {
              setIsCommandSession(false)
              setIsCommandMenuOpen(false)
            }
            commandTriggerRef.current = false
            setCommandIndex(0)
            updateDraftMessage(nextMessage)
          }}
          onCompositionStart={() => {
            isComposingRef.current = true
          }}
          onCompositionEnd={() => {
            isComposingRef.current = false
            lastCompositionEndAtRef.current = Date.now()
          }}
          onKeyDown={(event) => {
            if (isConfirmingImeInput(event)) return
            if (isMentionMenuOpen) {
              if (event.key === 'Escape') {
                event.preventDefault()
                setIsMentionMenuOpen(false)
                setMentionQuery('')
                updateDraftMessage('')
                return
              }
              if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault()
                const count =
                  mentionQuery.trim() === '' ? mentionHomeItemCount : mentionEntries.length
                if (count > 0)
                  setMentionIndex(
                    (mentionIndex + (event.key === 'ArrowDown' ? 1 : count - 1)) % count
                  )
                return
              }
              if (
                mentionQuery.trim() === '' &&
                (event.key === 'Enter' || event.key === 'Tab') &&
                mentionIndex < mentionHomeItemCount
              ) {
                event.preventDefault()
                activateMentionHomeItem(mentionIndex)
                return
              }
              if ((event.key === 'Enter' || event.key === 'Tab') && mentionEntries[mentionIndex]) {
                event.preventDefault()
                selectWorkspaceMention(mentionEntries[mentionIndex])
                return
              }
              if (event.key === 'Enter' || event.key === 'Tab') {
                event.preventDefault()
                return
              }
            }
            commandTriggerRef.current =
              event.key === '/' &&
              message === '' &&
              !event.ctrlKey &&
              !event.metaKey &&
              !event.altKey
            if (isCommandMenuOpen && event.key === 'Escape') {
              event.preventDefault()
              if (isCommandSubmenuOpen) returnToCommands()
              else setIsCommandMenuOpen(false)
              return
            }
            if (
              isCommandSubmenuOpen &&
              !isCapabilityDialogOpen &&
              (event.key === 'Backspace' || event.key === 'Delete') &&
              !event.ctrlKey &&
              !event.metaKey &&
              !event.altKey &&
              !event.shiftKey
            ) {
              event.preventDefault()
              returnToCommands()
              return
            }
            if (isCommandSubmenuOpen && event.key === 'Enter') {
              event.preventDefault()
              return
            }
            if (
              isCommandMenuOpen &&
              !isCommandSubmenuOpen &&
              (event.key === 'ArrowDown' || event.key === 'ArrowUp')
            ) {
              event.preventDefault()
              const count = filteredCommands.length
              if (count > 0)
                setCommandIndex(
                  (selectedCommandIndex + (event.key === 'ArrowDown' ? 1 : count - 1)) % count
                )
              return
            }
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
          <div
            className="composer-add-picker"
            ref={attachmentPickerRef}
            onKeyDown={handleAddMenuKeyDown}
          >
            <button
              ref={attachmentTriggerRef}
              type="button"
              className="composer-icon-button"
              aria-haspopup="menu"
              aria-expanded={isAttachmentMenuOpen}
              aria-label={t('chat.addContext')}
              disabled={isModelTransitionRunning}
              onClick={() => {
                setAttachmentError(null)
                setIsMentionMenuOpen(false)
                setMentionQuery('')
                setIsAttachmentMenuOpen((open) => {
                  if (!open) setAddMenuIndex(0)
                  return !open
                })
                setIsPermissionMenuOpen(false)
                setIsProjectMenuOpen(false)
              }}
            >
              <Plus aria-hidden="true" />
            </button>

            {isAttachmentMenuOpen && (
              <AnchoredPopover
                anchorRef={composerRef}
                className="chat-composer-menu-popover"
                enabled={portalMenus}
                matchAnchorWidth
                placement="top"
                returnFocusRef={attachmentTriggerRef}
                onClose={() => setIsAttachmentMenuOpen(false)}
                popoverRef={attachmentPopoverRef}
              >
                <ComposerAddMenu
                  addFileLabel={t('chat.addFile')}
                  addFolderLabel={t('chat.addFolder')}
                  addImageLabel={t('chat.addImage')}
                  addMenuTitle={t('chat.addMenuTitle')}
                  onAddFile={() => addAttachments('file')}
                  onAddFolder={addFolders}
                  onAddImage={() => addAttachments('image')}
                  selectedIndex={addMenuIndex}
                  onSelectIndex={setAddMenuIndex}
                  skills={!isGenerating ? addMenuSkills : undefined}
                  skillsTitle={t('chat.skills')}
                  skillLoading={skillCatalogState.status === 'loading'}
                  skillLoadingLabel={t('chat.loadingSkills')}
                  skillError={skillCatalogState.status === 'error'}
                  skillErrorLabel={t('chat.skillsLoadFailed')}
                  skillRetryLabel={t('chat.retrySkills')}
                  skillCatalogTruncated={Boolean(skillCatalog?.truncated)}
                  skillTruncatedLabel={t('chat.skillCatalogTruncated')}
                  skillDiagnosticsCount={skillCatalog?.diagnostics.length ?? 0}
                  skillDiagnosticsLabel={t('chat.skillDiagnostics')}
                  skillDiagnosticsAvailableLabel={t('skills.diagnosticsAvailable')}
                  skillEmptyLabel={t('chat.noSkills')}
                  onRetrySkills={refreshSkillCatalog}
                  onToggleSkill={(skill) => {
                    const descriptor = skillCatalogDescriptors.find((item) => item.id === skill.id)
                    if (descriptor) toggleSkill(descriptor)
                  }}
                />
              </AnchoredPopover>
            )}
          </div>

          <div className="composer-permission-picker" ref={permissionPickerRef}>
            <button
              type="button"
              className="composer-permission-button"
              ref={permissionTriggerRef}
              disabled={isModelSelectionDisabled}
              title={nextTurnConfigurationHint}
              data-permission={selectedPermission.id}
              aria-haspopup="listbox"
              aria-expanded={isPermissionMenuOpen}
              aria-label={`${t('chat.permission')}：${t(selectedPermission.labelKey)}`}
              onClick={() => {
                setIsAttachmentMenuOpen(false)
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
              <AnchoredPopover
                anchorRef={permissionTriggerRef}
                className="chat-composer-menu-popover"
                enabled={portalMenus}
                onClose={() => setIsPermissionMenuOpen(false)}
                popoverRef={permissionPopoverRef}
              >
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
                        disabled={isModelSelectionDisabled}
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
              </AnchoredPopover>
            )}
          </div>

          <span className="chat-composer__spacer" />

          {contextWindowIndicatorEnabled && contextWindowSnapshot && (
            <ContextWindowIndicator snapshot={contextWindowSnapshot} />
          )}

          <ModelConfigPicker
            ariaLabel={t('chat.selectModel')}
            disabled={isModelSelectionDisabled}
            emptyLabel={t('chat.noEnabledModels')}
            portalMenu={portalMenus}
            onChange={selectModelConfig}
            options={modelOptions}
            value={selectedModel?.id ?? null}
            title={nextTurnConfigurationHint}
            variant="composer"
          />

          <button
            type={submitButtonState === 'stop' ? 'button' : 'submit'}
            className="composer-submit-button"
            data-state={submitButtonState}
            disabled={submitButtonState === 'disabled'}
            aria-label={
              hasCommandSelection
                ? selectedCommand?.label
                : isGenerating
                  ? canSend
                    ? t('chat.queueMessage')
                    : t('chat.stop')
                  : t('chat.send')
            }
            onClick={() => {
              if (submitButtonState === 'stop') {
                onStopGenerating?.()
              }
            }}
          >
            {submitButtonState === 'stop' ? (
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
                ref={projectTriggerRef}
                type="button"
                disabled={isModelTransitionRunning}
                aria-haspopup="listbox"
                aria-expanded={isProjectMenuOpen}
                onClick={() => {
                  setIsAttachmentMenuOpen(false)
                  setIsPermissionMenuOpen(false)
                  setIsProjectMenuOpen((open) => !open)
                }}
              >
                <Folder aria-hidden="true" />
                <span title={selectedProject?.name}>
                  {selectedProject?.name ?? t('project.chooseProject')}
                </span>
              </button>
              {selectedProject && (
                <button
                  className="composer-project-clear"
                  type="button"
                  disabled={isModelTransitionRunning}
                  aria-label={t('project.noProject')}
                  onClick={clearSelectedProject}
                >
                  <X aria-hidden="true" />
                </button>
              )}

              {isProjectMenuOpen && (
                <AnchoredPopover
                  anchorRef={projectTriggerRef}
                  enabled={portalMenus}
                  onClose={() => setIsProjectMenuOpen(false)}
                  popoverRef={projectPopoverRef}
                >
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
                      onClick={clearSelectedProject}
                    >
                      <X aria-hidden="true" />
                      <span>{t('project.noProject')}</span>
                    </button>
                  </div>
                </AnchoredPopover>
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
              if (permissionModeAvailability.full && !isModelSelectionDisabled) {
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
