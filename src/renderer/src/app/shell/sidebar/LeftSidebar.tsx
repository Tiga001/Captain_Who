import {
  ChevronDown,
  Clock3,
  Folder,
  FolderOpen,
  MoreHorizontal,
  Plus,
  Search,
  SquarePen
} from 'lucide-react'
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { isAssistantMessageGenerating } from '../../../features/chat/assistantGeneration'
import type {
  SidebarConversationSort,
  SidebarProjectSort
} from '../../../features/storage/storageClient'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import { CaptainWhoWordmark } from './CaptainWhoWordmark'
import { LeftSidebarAccountFooter } from './LeftSidebarAccountFooter'
import { ConversationRow } from './LeftSidebarConversationRow'
import { LeftSidebarDialogs } from './LeftSidebarDialogs'
import { LeftSidebarProjectMenu } from './LeftSidebarProjectMenu'
import { LeftSidebarSearchDialog } from './LeftSidebarSearchDialog'
import { LeftSidebarSectionMenu } from './LeftSidebarSectionMenu'
import type {
  BulkArchiveScope,
  ConversationListStage,
  LeftSidebarProps,
  ProjectDragPosition,
  ProjectDragTarget,
  ProjectPointerDragState,
  SidebarConversation,
  SidebarMenuPosition,
  SidebarSectionScope,
  SidebarSectionSubmenu
} from './leftSidebarTypes'
import {
  COLLAPSED_CONVERSATION_COUNT,
  PREVIEW_CONVERSATION_COUNT,
  PREVIEW_CONVERSATION_THRESHOLD,
  ROOT_CONVERSATION_LIST_KEY,
  SIDEBAR_PROJECT_MENU_ESTIMATED_HEIGHT,
  SIDEBAR_SECTION_MENU_ESTIMATED_HEIGHT,
  clampMenuPosition,
  sortConversations,
  sortPinnedProjects,
  sortProjectsByManualOrder,
  sortRegularProjects
} from './leftSidebarUtils'
import './LeftSidebar.css'

const sidebarActivityCache = new WeakMap<
  ChatConversation,
  ReturnType<typeof computeSidebarActivity>
>()

function computeSidebarActivity(conversation: ChatConversation) {
  let isPending = false
  let isWaitingForApproval = false

  for (let index = conversation.messages.length - 1; index >= 0; index -= 1) {
    const message = conversation.messages[index]
    isPending ||= isAssistantMessageGenerating(message)
    isWaitingForApproval ||=
      message.role === 'assistant' && message.agentRun?.status === 'waiting_for_approval'
    if (isPending && isWaitingForApproval) break
  }

  return { isPending, isWaitingForApproval }
}

function getSidebarConversationActivity(conversation: ChatConversation) {
  const cached = sidebarActivityCache.get(conversation)
  if (cached) return cached
  const activity = computeSidebarActivity(conversation)
  sidebarActivityCache.set(conversation, activity)
  return activity
}

function useLatestCallback<Args extends unknown[], Result>(
  callback: (...args: Args) => Result
): (...args: Args) => Result {
  const callbackRef = useRef(callback)

  useLayoutEffect(() => {
    callbackRef.current = callback
  }, [callback])

  return useCallback((...args: Args) => callbackRef.current(...args), [])
}

export function LeftSidebar(props: LeftSidebarProps) {
  const conversationSnapshot = JSON.stringify(
    props.conversations.map((conversation): SidebarConversation => {
      const activity = getSidebarConversationActivity(conversation)
      return {
        archivedAt: conversation.archivedAt,
        createdAt: conversation.createdAt,
        id: conversation.id,
        ...activity,
        pinnedAt: conversation.pinnedAt,
        projectId: conversation.projectId,
        title: conversation.title,
        unreadAt: conversation.unreadAt,
        updatedAt: conversation.updatedAt
      }
    })
  )
  const conversations = useMemo(
    () => JSON.parse(conversationSnapshot) as SidebarConversation[],
    [conversationSnapshot]
  )
  const onArchiveAllProjectConversations = useLatestCallback(props.onArchiveAllProjectConversations)
  const onArchiveAllRootConversations = useLatestCallback(props.onArchiveAllRootConversations)
  const onArchiveConversation = useLatestCallback(props.onArchiveConversation)
  const onArchiveProjectConversations = useLatestCallback(props.onArchiveProjectConversations)
  const onEditProject = useLatestCallback(props.onEditProject)
  const onMarkConversationUnread = useLatestCallback(props.onMarkConversationUnread)
  const onNewConversation = useLatestCallback(props.onNewConversation)
  const onNewProject = useLatestCallback(props.onNewProject)
  const onOpenSettings = useLatestCallback(props.onOpenSettings)
  const onRemoveProject = useLatestCallback(props.onRemoveProject)
  const onRenameConversation = useLatestCallback(props.onRenameConversation)
  const onOpenScheduled = useLatestCallback(props.onOpenScheduled)
  const onSelectConversation = useLatestCallback(props.onSelectConversation)
  const onShowProjectInFolder = useLatestCallback(props.onShowProjectInFolder)
  const onTogglePinConversation = useLatestCallback(props.onTogglePinConversation)
  const onTogglePinProject = useLatestCallback(props.onTogglePinProject)
  const onUiPreferencesChange = useLatestCallback(props.onUiPreferencesChange)

  return (
    <LeftSidebarView
      {...props}
      conversations={conversations}
      onArchiveAllProjectConversations={onArchiveAllProjectConversations}
      onArchiveAllRootConversations={onArchiveAllRootConversations}
      onArchiveConversation={onArchiveConversation}
      onArchiveProjectConversations={onArchiveProjectConversations}
      onEditProject={onEditProject}
      onMarkConversationUnread={onMarkConversationUnread}
      onNewConversation={onNewConversation}
      onNewProject={onNewProject}
      onOpenSettings={onOpenSettings}
      onRemoveProject={onRemoveProject}
      onRenameConversation={onRenameConversation}
      onOpenScheduled={onOpenScheduled}
      onSelectConversation={onSelectConversation}
      onShowProjectInFolder={onShowProjectInFolder}
      onTogglePinConversation={onTogglePinConversation}
      onTogglePinProject={onTogglePinProject}
      onUiPreferencesChange={onUiPreferencesChange}
    />
  )
}

type LeftSidebarViewProps = Omit<LeftSidebarProps, 'conversations'> & {
  conversations: SidebarConversation[]
}

const LeftSidebarView = memo(function LeftSidebarView({
  activeConversationId,
  conversations,
  onArchiveAllProjectConversations,
  onArchiveAllRootConversations,
  onArchiveConversation,
  onArchiveProjectConversations,
  onEditProject,
  onMarkConversationUnread,
  onNewConversation,
  onNewProject,
  onOpenSettings,
  onRemoveProject,
  onRequestRenameConversation,
  onRenameConversation,
  onOpenScheduled,
  onSelectConversation,
  onShowProjectInFolder,
  onTogglePinConversation,
  onTogglePinProject,
  onUiPreferencesChange,
  projects,
  scheduledAttentionCount,
  scheduledSelected,
  uiPreferences
}: LeftSidebarViewProps) {
  const { language, t } = useFrontendConfig()
  const [areProjectsOpen, setAreProjectsOpen] = useState(true)
  const [areConversationsOpen, setAreConversationsOpen] = useState(true)
  const [openProjectIds, setOpenProjectIds] = useState<Set<string>>(new Set())
  const [openProjectMenuId, setOpenProjectMenuId] = useState<string | null>(null)
  const [projectMenuPosition, setProjectMenuPosition] = useState<SidebarMenuPosition | null>(null)
  const [openSectionMenu, setOpenSectionMenu] = useState<SidebarSectionScope | null>(null)
  const [sectionMenuPosition, setSectionMenuPosition] = useState<SidebarMenuPosition | null>(null)
  const [openSectionSubmenu, setOpenSectionSubmenu] = useState<SidebarSectionSubmenu | null>(null)
  const [renamingConversation, setRenamingConversation] = useState<SidebarConversation | null>(null)
  const [conversationRenameValue, setConversationRenameValue] = useState('')
  const [pendingBulkArchiveScope, setPendingBulkArchiveScope] = useState<BulkArchiveScope | null>(
    null
  )
  const [pendingArchiveProject, setPendingArchiveProject] = useState<AppProject | null>(null)
  const [pendingRemoveProject, setPendingRemoveProject] = useState<AppProject | null>(null)
  const [isSearchDialogOpen, setIsSearchDialogOpen] = useState(false)
  const [conversationListStages, setConversationListStages] = useState<
    Record<string, ConversationListStage>
  >({})
  const [draggingProjectId, setDraggingProjectId] = useState<string | null>(null)
  const [projectDragPreviewOrder, setProjectDragPreviewOrderState] = useState<string[] | null>(null)
  const [now, setNow] = useState(() => Date.now())
  const projectMenuRef = useRef<HTMLDivElement>(null)
  const sectionMenuRef = useRef<HTMLDivElement>(null)
  const projectRowRefs = useRef<Map<string, HTMLDivElement>>(new Map())
  const projectAnimationRectsRef = useRef<Map<string, DOMRect> | null>(null)
  const projectPointerDragRef = useRef<ProjectPointerDragState | null>(null)
  const projectDragPreviewOrderRef = useRef<string[] | null>(null)
  const suppressProjectClickRef = useRef<string | null>(null)
  const sidebarProjection = useMemo(() => {
    const projectIds = new Set(projects.map((project) => project.id))
    const conversationsByProjectId = Object.fromEntries(
      projects.map((project) => [project.id, [] as SidebarConversation[]])
    )
    const visibleConversations: SidebarConversation[] = []
    const pinnedRootConversations: SidebarConversation[] = []
    const rootConversations: SidebarConversation[] = []
    let activeConversation: SidebarConversation | undefined
    let projectArchiveAllCount = 0
    let rootArchiveAllCount = 0

    for (const conversation of conversations) {
      if (conversation.archivedAt) continue
      visibleConversations.push(conversation)
      if (conversation.id === activeConversationId) activeConversation = conversation

      const projectId = conversation.projectId
      if (projectId && projectIds.has(projectId)) {
        conversationsByProjectId[projectId].push(conversation)
        projectArchiveAllCount += 1
      } else {
        if (conversation.pinnedAt) pinnedRootConversations.push(conversation)
        else rootConversations.push(conversation)
        rootArchiveAllCount += 1
      }
    }

    for (const [projectId, projectConversations] of Object.entries(conversationsByProjectId)) {
      conversationsByProjectId[projectId] = sortConversations(
        projectConversations,
        uiPreferences.sidebarConversationSort
      )
    }

    const pinnedProjects = sortPinnedProjects(projects)
    const regularProjects = sortRegularProjects(
      projects,
      conversationsByProjectId,
      uiPreferences.sidebarProjectSort,
      uiPreferences.sidebarProjectOrder
    )

    return {
      activeConversation,
      conversationsByProjectId,
      pinnedProjects,
      pinnedRootConversations: sortConversations(
        pinnedRootConversations,
        uiPreferences.sidebarConversationSort
      ),
      projectArchiveAllCount,
      regularProjects,
      rootArchiveAllCount,
      rootConversations: sortConversations(
        rootConversations,
        uiPreferences.sidebarConversationSort
      ),
      visibleConversations
    }
  }, [
    activeConversationId,
    conversations,
    projects,
    uiPreferences.sidebarConversationSort,
    uiPreferences.sidebarProjectOrder,
    uiPreferences.sidebarProjectSort
  ])
  const {
    activeConversation,
    conversationsByProjectId,
    pinnedProjects,
    pinnedRootConversations,
    projectArchiveAllCount,
    regularProjects,
    rootArchiveAllCount,
    rootConversations,
    visibleConversations
  } = sidebarProjection
  const displayRegularProjects = useMemo(
    () =>
      projectDragPreviewOrder
        ? sortProjectsByManualOrder(regularProjects, projectDragPreviewOrder)
        : regularProjects,
    [projectDragPreviewOrder, regularProjects]
  )
  const hasPinnedItems = pinnedProjects.length > 0 || pinnedRootConversations.length > 0
  const bulkArchiveCount =
    pendingBulkArchiveScope === 'projects' ? projectArchiveAllCount : rootArchiveAllCount

  const closeProjectMenu = () => {
    setOpenProjectMenuId(null)
    setProjectMenuPosition(null)
  }

  useDismissOnOutsidePointer(projectMenuRef, Boolean(openProjectMenuId), closeProjectMenu)
  useDismissOnOutsidePointer(sectionMenuRef, Boolean(openSectionMenu), () => {
    setOpenSectionMenu(null)
    setSectionMenuPosition(null)
    setOpenSectionSubmenu(null)
  })

  useEffect(() => {
    const intervalId = window.setInterval(() => setNow(Date.now()), 60_000)
    return () => window.clearInterval(intervalId)
  }, [])

  useEffect(() => {
    if (!activeConversation?.projectId) return

    setOpenProjectIds((currentIds) => {
      if (currentIds.has(activeConversation.projectId!)) return currentIds
      const nextIds = new Set(currentIds)
      nextIds.add(activeConversation.projectId!)
      return nextIds
    })
  }, [activeConversation?.projectId])

  useLayoutEffect(() => {
    const previousRects = projectAnimationRectsRef.current
    if (!previousRects) return

    projectAnimationRectsRef.current = null

    for (const project of displayRegularProjects) {
      const element = projectRowRefs.current.get(project.id)
      const previousRect = previousRects.get(project.id)
      if (!element || !previousRect) continue

      const nextRect = element.getBoundingClientRect()
      const deltaY = previousRect.top - nextRect.top
      if (Math.abs(deltaY) < 1) continue

      element.animate([{ transform: `translateY(${deltaY}px)` }, { transform: 'translateY(0)' }], {
        duration: 150,
        easing: 'cubic-bezier(0.2, 0, 0, 1)'
      })
    }
  }, [displayRegularProjects])

  const toggleProject = (projectId: string) => {
    setOpenProjectIds((currentIds) => {
      const nextIds = new Set(currentIds)
      if (nextIds.has(projectId)) {
        nextIds.delete(projectId)
      } else {
        nextIds.add(projectId)
      }
      return nextIds
    })
  }

  const startEditingProject = (project: AppProject) => {
    closeProjectMenu()
    void onEditProject(project.id).then((result) => {
      // The editor's "remove local project" entry hands off to the same confirmation flow the
      // context menu uses, so removal stays a single, explicit decision.
      if (result === 'remove-requested') setPendingRemoveProject(project)
    })
  }

  const startRenamingConversation = useCallback(
    (conversation: SidebarConversation) => {
      if (onRequestRenameConversation) {
        onRequestRenameConversation(conversation.id)
        return
      }
      setConversationRenameValue(conversation.title)
      setRenamingConversation(conversation)
    },
    [onRequestRenameConversation]
  )

  const confirmRenameConversation = () => {
    if (!renamingConversation) return

    const normalizedTitle = conversationRenameValue.trim()
    if (!normalizedTitle) return

    onRenameConversation(renamingConversation.id, normalizedTitle)
    setRenamingConversation(null)
    setConversationRenameValue('')
  }

  const closeSectionMenu = () => {
    setOpenSectionMenu(null)
    setSectionMenuPosition(null)
    setOpenSectionSubmenu(null)
  }

  const openSearchDialog = () => {
    closeProjectMenu()
    closeSectionMenu()
    setIsSearchDialogOpen(true)
  }

  const openSectionActions = (scope: SidebarSectionScope, anchorElement: HTMLElement) => {
    if (openSectionMenu === scope) {
      closeSectionMenu()
      return
    }

    const sectionElement = anchorElement.closest('.left-sidebar__section')
    const sectionRect =
      sectionElement instanceof HTMLElement
        ? sectionElement.getBoundingClientRect()
        : anchorElement.getBoundingClientRect()

    setOpenSectionMenu(scope)
    setSectionMenuPosition(
      clampMenuPosition(
        sectionRect.left,
        sectionRect.top + 34,
        SIDEBAR_SECTION_MENU_ESTIMATED_HEIGHT
      )
    )
    setOpenSectionSubmenu(null)
    closeProjectMenu()
  }

  const openProjectActions = (
    projectId: string,
    anchorElement: HTMLElement,
    pointerPosition?: SidebarMenuPosition
  ) => {
    closeSectionMenu()

    if (!pointerPosition && openProjectMenuId === projectId) {
      closeProjectMenu()
      return
    }

    const rowElement = anchorElement.closest('.left-sidebar__project-row')
    const rowRect =
      rowElement instanceof HTMLElement
        ? rowElement.getBoundingClientRect()
        : anchorElement.getBoundingClientRect()
    const nextPosition = pointerPosition
      ? clampMenuPosition(
          pointerPosition.left,
          pointerPosition.top,
          SIDEBAR_PROJECT_MENU_ESTIMATED_HEIGHT
        )
      : clampMenuPosition(
          rowRect.left + 42,
          rowRect.bottom + 1,
          SIDEBAR_PROJECT_MENU_ESTIMATED_HEIGHT
        )

    setProjectMenuPosition(nextPosition)
    setOpenProjectMenuId(projectId)
  }

  const setConversationSort = (sort: SidebarConversationSort) => {
    onUiPreferencesChange({ sidebarConversationSort: sort })
    closeSectionMenu()
  }

  const setProjectSort = (sort: SidebarProjectSort) => {
    onUiPreferencesChange({ sidebarProjectSort: sort })
    closeSectionMenu()
  }

  const buildProjectOrderWithSection = (
    sectionProjectIds: string[],
    nextSectionProjectIds: string[]
  ) => {
    const allProjectIds = projects.map((project) => project.id)
    const sectionIdSet = new Set(sectionProjectIds)
    const knownIdSet = new Set(allProjectIds)
    const currentOrder = [
      ...uiPreferences.sidebarProjectOrder.filter((projectId) => knownIdSet.has(projectId)),
      ...allProjectIds.filter((projectId) => !uiPreferences.sidebarProjectOrder.includes(projectId))
    ]
    const preservedIds = currentOrder.filter((projectId) => !sectionIdSet.has(projectId))

    return [...preservedIds, ...nextSectionProjectIds]
  }

  const setProjectDragPreviewOrder = (order: string[] | null) => {
    projectDragPreviewOrderRef.current = order
    setProjectDragPreviewOrderState(order)
  }

  const captureProjectRowRects = (projectIds: string[]) => {
    const rects = new Map<string, DOMRect>()

    for (const projectId of projectIds) {
      const element = projectRowRefs.current.get(projectId)
      if (element) rects.set(projectId, element.getBoundingClientRect())
    }

    projectAnimationRectsRef.current = rects
  }

  const resetProjectPointerDrag = () => {
    projectPointerDragRef.current = null
    setDraggingProjectId(null)
    setProjectDragPreviewOrder(null)
  }

  const areProjectOrdersEqual = (a: string[], b: string[]) =>
    a.length === b.length && a.every((projectId, index) => projectId === b[index])

  const buildMovedProjectOrder = (
    draggedProjectId: string,
    targetProjectId: string,
    position: ProjectDragPosition,
    sectionProjectIds: string[]
  ) => {
    if (draggedProjectId === targetProjectId) return sectionProjectIds
    if (
      !sectionProjectIds.includes(draggedProjectId) ||
      !sectionProjectIds.includes(targetProjectId)
    ) {
      return sectionProjectIds
    }

    const nextSectionProjectIds = sectionProjectIds.filter(
      (projectId) => projectId !== draggedProjectId
    )
    const targetIndex = nextSectionProjectIds.indexOf(targetProjectId)
    nextSectionProjectIds.splice(
      position === 'after' ? targetIndex + 1 : targetIndex,
      0,
      draggedProjectId
    )

    return nextSectionProjectIds
  }

  const getProjectDragTargetFromPoint = (
    clientX: number,
    clientY: number,
    sectionProjectIds: string[]
  ): ProjectDragTarget | null => {
    const targetElement = document.elementFromPoint(clientX, clientY)
    const projectElement = targetElement?.closest('[data-project-id]')

    if (!(projectElement instanceof HTMLElement)) return null

    const projectId = projectElement.dataset.projectId
    if (!projectId || !sectionProjectIds.includes(projectId)) return null

    const rect = projectElement.getBoundingClientRect()
    return {
      projectId,
      position: clientY > rect.top + rect.height / 2 ? 'after' : 'before'
    }
  }

  const handleProjectPointerDown = (
    event: ReactPointerEvent<HTMLButtonElement>,
    project: AppProject,
    sectionProjects: AppProject[],
    isProjectOpen: boolean
  ) => {
    if (event.button !== 0 || isProjectOpen || sectionProjects.length <= 1) return
    if (
      (event.target as HTMLElement).closest(
        '.left-sidebar__project-icon, .left-sidebar__project-inline-chevron'
      )
    ) {
      return
    }

    closeProjectMenu()
    closeSectionMenu()
    projectPointerDragRef.current = {
      hasMoved: false,
      pointerId: event.pointerId,
      projectId: project.id,
      sectionProjectIds: sectionProjects.map((sectionProject) => sectionProject.id),
      startY: event.clientY
    }
    setProjectDragPreviewOrder(null)
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  const handleProjectPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current
    if (!dragState || dragState.pointerId !== event.pointerId) return

    if (!dragState.hasMoved && Math.abs(event.clientY - dragState.startY) < 5) return

    dragState.hasMoved = true
    suppressProjectClickRef.current = dragState.projectId
    setDraggingProjectId(dragState.projectId)

    const currentPreviewOrder = projectDragPreviewOrderRef.current ?? dragState.sectionProjectIds
    const nextTarget = getProjectDragTargetFromPoint(
      event.clientX,
      event.clientY,
      currentPreviewOrder
    )

    if (nextTarget && nextTarget.projectId !== dragState.projectId) {
      const nextPreviewOrder = buildMovedProjectOrder(
        dragState.projectId,
        nextTarget.projectId,
        nextTarget.position,
        currentPreviewOrder
      )

      if (!areProjectOrdersEqual(currentPreviewOrder, nextPreviewOrder)) {
        captureProjectRowRects(currentPreviewOrder)
        setProjectDragPreviewOrder(nextPreviewOrder)
      }
    }

    event.preventDefault()
  }

  const handleProjectPointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current
    if (!dragState || dragState.pointerId !== event.pointerId) return

    const finalPreviewOrder = projectDragPreviewOrderRef.current
    if (
      dragState.hasMoved &&
      finalPreviewOrder &&
      !areProjectOrdersEqual(dragState.sectionProjectIds, finalPreviewOrder)
    ) {
      onUiPreferencesChange({
        sidebarProjectSort: 'manual',
        sidebarProjectOrder: buildProjectOrderWithSection(
          dragState.sectionProjectIds,
          finalPreviewOrder
        )
      })
    }

    resetProjectPointerDrag()
    window.setTimeout(() => {
      suppressProjectClickRef.current = null
    }, 0)
  }

  const handleProjectPointerCancel = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const dragState = projectPointerDragRef.current
    if (!dragState || dragState.pointerId !== event.pointerId) return

    resetProjectPointerDrag()
    window.setTimeout(() => {
      suppressProjectClickRef.current = null
    }, 0)
  }

  const isSectionFirst = (scope: SidebarSectionScope) =>
    scope === 'projects'
      ? uiPreferences.sidebarSectionOrder === 'projects_first'
      : uiPreferences.sidebarSectionOrder === 'conversations_first'

  const toggleSectionOrder = (scope: SidebarSectionScope) => {
    const nextOrder =
      scope === 'projects'
        ? isSectionFirst(scope)
          ? 'conversations_first'
          : 'projects_first'
        : isSectionFirst(scope)
          ? 'projects_first'
          : 'conversations_first'
    onUiPreferencesChange({ sidebarSectionOrder: nextOrder })
    closeSectionMenu()
  }

  const archiveProjectCount = pendingArchiveProject
    ? (conversationsByProjectId[pendingArchiveProject.id]?.length ?? 0)
    : 0

  const renderConversationRow = (conversation: SidebarConversation, nested = false) => (
    <ConversationRow
      activeConversationId={activeConversationId}
      archiveLabel={t('conversation.archiveConversation')}
      conversation={conversation}
      key={conversation.id}
      language={language}
      markUnreadLabel={t('conversation.markUnread')}
      nested={nested}
      now={now}
      onArchiveConversation={onArchiveConversation}
      onMarkConversationUnread={onMarkConversationUnread}
      onRenameConversation={startRenamingConversation}
      onSelectConversation={onSelectConversation}
      onTogglePinConversation={onTogglePinConversation}
      pinLabel={t('conversation.pinConversation')}
      processingLabel={t('sidebar.processingConversation')}
      renameLabel={t('conversation.renameConversation')}
      unreadLabel={t('sidebar.unreadConversation')}
      unpinLabel={t('conversation.unpinConversation')}
      waitingApprovalLabel={t('sidebar.waitingApproval')}
      justNow={t('sidebar.justNow')}
    />
  )

  const renderConversationCollection = (
    listKey: string,
    collection: SidebarConversation[],
    nested = false
  ) => {
    const stage = conversationListStages[listKey] ?? 'collapsed'
    const shouldPaginate = collection.length > 7
    const visibleCount =
      !shouldPaginate || stage === 'expanded'
        ? collection.length
        : stage === 'preview'
          ? PREVIEW_CONVERSATION_COUNT
          : COLLAPSED_CONVERSATION_COUNT
    const visibleCollection = collection.slice(0, visibleCount)
    const canExpand = visibleCollection.length < collection.length
    const canCollapse = shouldPaginate && stage !== 'collapsed'

    const expandCollection = () => {
      setConversationListStages((currentStages) => ({
        ...currentStages,
        [listKey]:
          stage === 'collapsed' && collection.length > PREVIEW_CONVERSATION_THRESHOLD
            ? 'preview'
            : 'expanded'
      }))
    }

    const collapseCollection = () => {
      setConversationListStages((currentStages) => ({
        ...currentStages,
        [listKey]: 'collapsed'
      }))
    }

    return (
      <>
        {visibleCollection.map((conversation) => renderConversationRow(conversation, nested))}
        {shouldPaginate && (canExpand || canCollapse) && (
          <div className="left-sidebar__conversation-disclosure" data-nested={nested || undefined}>
            {canExpand && (
              <button type="button" onClick={expandCollection}>
                {t('sidebar.expandConversations')}
              </button>
            )}
            {canCollapse && (
              <button type="button" onClick={collapseCollection}>
                {t('sidebar.collapseConversations')}
              </button>
            )}
          </div>
        )}
      </>
    )
  }

  const renderSectionMenu = (scope: SidebarSectionScope) => (
    <LeftSidebarSectionMenu
      archiveCount={scope === 'projects' ? projectArchiveAllCount : rootArchiveAllCount}
      archiveScope={scope === 'projects' ? 'projects' : 'root'}
      conversationSort={uiPreferences.sidebarConversationSort}
      isMoveDown={isSectionFirst(scope)}
      menuPosition={sectionMenuPosition}
      onRequestBulkArchive={(archiveScope) => {
        setPendingBulkArchiveScope(archiveScope)
        closeSectionMenu()
      }}
      onSetConversationSort={setConversationSort}
      onSetProjectSort={setProjectSort}
      onSubmenuChange={setOpenSectionSubmenu}
      onToggleSectionOrder={() => toggleSectionOrder(scope)}
      openSubmenu={openSectionSubmenu}
      projectSort={uiPreferences.sidebarProjectSort}
      sectionMenuRef={sectionMenuRef}
      t={t}
    />
  )

  const renderProjectGroup = (
    project: AppProject,
    pinnedSection = false,
    sectionProjects: AppProject[]
  ) => {
    const isProjectOpen = openProjectIds.has(project.id)
    const isProjectPinned = Boolean(project.pinnedAt)
    const ProjectIcon = isProjectOpen ? FolderOpen : Folder
    const projectConversations = conversationsByProjectId[project.id] ?? []
    const visibleProjectConversationCount = projectConversations.length
    const isDragging = draggingProjectId === project.id
    const canDragProject = !isProjectOpen && sectionProjects.length > 1
    const hasPendingProjectConversation = projectConversations.some(
      (conversation) => conversation.isPending
    )
    const hasUnreadProjectConversation = projectConversations.some(
      (conversation) => conversation.unreadAt && conversation.id !== activeConversationId
    )
    const shouldShowProjectStatus =
      !isProjectOpen && (hasPendingProjectConversation || hasUnreadProjectConversation)

    return (
      <div
        className={`left-sidebar__project-group${pinnedSection ? ' left-sidebar__project-group--pinned' : ''}`}
        data-dragging={isDragging || undefined}
        key={project.id}
      >
        <div
          className="left-sidebar__project-row"
          data-project-id={project.id}
          ref={(element) => {
            if (element) {
              projectRowRefs.current.set(project.id, element)
            } else {
              projectRowRefs.current.delete(project.id)
            }
          }}
          onContextMenu={(event) => {
            event.preventDefault()
            openProjectActions(project.id, event.currentTarget, {
              left: event.clientX,
              top: event.clientY
            })
          }}
        >
          <button
            className="left-sidebar__project-main"
            type="button"
            data-draggable={canDragProject || undefined}
            onClick={(event) => {
              if (suppressProjectClickRef.current === project.id) {
                event.preventDefault()
                return
              }

              toggleProject(project.id)
            }}
            onPointerCancel={handleProjectPointerCancel}
            onPointerDown={(event) =>
              handleProjectPointerDown(event, project, sectionProjects, isProjectOpen)
            }
            onPointerMove={handleProjectPointerMove}
            onPointerUp={handleProjectPointerUp}
          >
            <ProjectIcon className="left-sidebar__project-icon" aria-hidden="true" />
            <span>{project.name}</span>
            <ChevronDown
              className="left-sidebar__project-inline-chevron"
              data-open={isProjectOpen || undefined}
              aria-hidden="true"
            />
          </button>

          {shouldShowProjectStatus && (
            <span className="left-sidebar__project-status">
              {hasPendingProjectConversation ? (
                <span
                  className="mc-processing-spinner"
                  aria-label={t('sidebar.processingConversation')}
                />
              ) : (
                <span
                  className="left-sidebar__conversation-unread-dot"
                  aria-label={t('sidebar.unreadConversation')}
                />
              )}
            </span>
          )}

          <div className="left-sidebar__project-actions">
            <button
              className="left-sidebar__project-action left-sidebar__project-action--more"
              type="button"
              aria-label={t('project.moreActions')}
              data-menu-open={openProjectMenuId === project.id || undefined}
              onClick={(event) => openProjectActions(project.id, event.currentTarget)}
            >
              <MoreHorizontal aria-hidden="true" />
            </button>
            <button
              className="left-sidebar__project-action"
              type="button"
              aria-label={t('project.newProjectConversation')}
              onClick={() => onNewConversation(project.id)}
            >
              <SquarePen aria-hidden="true" />
            </button>
          </div>
        </div>

        {openProjectMenuId === project.id && projectMenuPosition && (
          <LeftSidebarProjectMenu
            isProjectPinned={isProjectPinned}
            onArchiveConversations={() => {
              setPendingArchiveProject(project)
              closeProjectMenu()
            }}
            onRemoveProject={() => {
              setPendingRemoveProject(project)
              closeProjectMenu()
            }}
            onEditProject={() => startEditingProject(project)}
            onShowInFolder={() => {
              onShowProjectInFolder(project.id)
              closeProjectMenu()
            }}
            onTogglePinProject={() => {
              onTogglePinProject(project.id)
              closeProjectMenu()
            }}
            project={project}
            projectMenuPosition={projectMenuPosition}
            projectMenuRef={projectMenuRef}
            t={t}
            visibleProjectConversationCount={visibleProjectConversationCount}
          />
        )}

        {isProjectOpen && projectConversations.length > 0 && (
          <div className="left-sidebar__project-conversations">
            {renderConversationCollection(`project:${project.id}`, projectConversations, true)}
          </div>
        )}
      </div>
    )
  }

  const projectsSection = (
    <section
      key="projects"
      className="left-sidebar__section left-sidebar__projects"
      aria-labelledby="projects-heading"
    >
      <div className="left-sidebar__section-header">
        <button
          className="left-sidebar__section-title-button"
          type="button"
          aria-expanded={areProjectsOpen}
          onClick={() => setAreProjectsOpen((open) => !open)}
        >
          <h2 id="projects-heading" className="left-sidebar__section-title">
            {t('sidebar.projects')}
          </h2>
          <ChevronDown data-open={areProjectsOpen || undefined} aria-hidden="true" />
        </button>

        <div className="left-sidebar__section-actions">
          <button
            className="left-sidebar__section-action"
            type="button"
            aria-label={t('project.moreActions')}
            data-open={openSectionMenu === 'projects' || undefined}
            onClick={(event) => openSectionActions('projects', event.currentTarget)}
          >
            <MoreHorizontal aria-hidden="true" />
          </button>
          <button
            className="left-sidebar__section-action"
            type="button"
            aria-label={t('project.newProject')}
            onClick={() => {
              closeProjectMenu()
              closeSectionMenu()
              void onNewProject()
            }}
          >
            <Plus aria-hidden="true" />
          </button>
        </div>
      </div>

      {openSectionMenu === 'projects' && sectionMenuPosition && renderSectionMenu('projects')}

      {areProjectsOpen && displayRegularProjects.length > 0 && (
        <div className="left-sidebar__project-list">
          {displayRegularProjects.map((project) =>
            renderProjectGroup(project, false, displayRegularProjects)
          )}
        </div>
      )}
    </section>
  )

  const conversationsSection = (
    <section
      key="conversations"
      className="left-sidebar__section left-sidebar__conversations"
      aria-labelledby="conversations-heading"
    >
      <div className="left-sidebar__conversation-header">
        <button
          className="left-sidebar__conversation-title-button"
          type="button"
          aria-expanded={areConversationsOpen}
          onClick={() => setAreConversationsOpen((open) => !open)}
        >
          <h2 id="conversations-heading" className="left-sidebar__section-title">
            {t('sidebar.conversations')}
          </h2>
          <ChevronDown data-open={areConversationsOpen || undefined} aria-hidden="true" />
        </button>

        <div className="left-sidebar__conversation-actions">
          <button
            className="left-sidebar__conversation-action"
            type="button"
            aria-label={t('sidebar.moreConversationActions')}
            data-open={openSectionMenu === 'conversations' || undefined}
            onClick={(event) => openSectionActions('conversations', event.currentTarget)}
          >
            <MoreHorizontal aria-hidden="true" />
          </button>
          <button
            className="left-sidebar__conversation-action"
            type="button"
            aria-label={t('sidebar.newConversationAction')}
            onClick={() => onNewConversation(null)}
          >
            <SquarePen aria-hidden="true" />
          </button>
        </div>
      </div>

      {openSectionMenu === 'conversations' &&
        sectionMenuPosition &&
        renderSectionMenu('conversations')}

      {areConversationsOpen &&
        (rootConversations.length === 0 ? (
          <p className="left-sidebar__empty-state">{t('sidebar.emptyConversations')}</p>
        ) : (
          <div className="left-sidebar__conversation-list">
            {renderConversationCollection(ROOT_CONVERSATION_LIST_KEY, rootConversations)}
          </div>
        ))}
    </section>
  )

  return (
    <aside
      className="left-sidebar"
      aria-label={t('app.leftSidebar')}
      onContextMenu={(event) => {
        if (!event.defaultPrevented) event.preventDefault()
      }}
    >
      <div className="left-sidebar__header">
        <div className="left-sidebar__brand">
          <CaptainWhoWordmark />
        </div>
        <button
          className="left-sidebar__primary-action"
          type="button"
          onClick={() => onNewConversation(null)}
        >
          <SquarePen aria-hidden="true" />
          <span>{t('sidebar.newConversation')}</span>
        </button>
        <button className="left-sidebar__search-action" type="button" onClick={openSearchDialog}>
          <Search aria-hidden="true" />
          <span>{t('sidebar.search')}</span>
        </button>
        <button
          className="left-sidebar__scheduled-action"
          type="button"
          aria-current={scheduledSelected ? 'page' : undefined}
          aria-label={
            scheduledAttentionCount > 0
              ? `${t('sidebar.scheduled')} (${Math.min(scheduledAttentionCount, 99)}${scheduledAttentionCount > 99 ? '+' : ''})`
              : t('sidebar.scheduled')
          }
          data-selected={scheduledSelected || undefined}
          onClick={onOpenScheduled}
        >
          <Clock3 aria-hidden="true" />
          <span>{t('sidebar.scheduled')}</span>
          {scheduledAttentionCount > 0 && (
            <span className="left-sidebar__scheduled-badge" aria-hidden="true">
              {scheduledAttentionCount > 99 ? '99+' : scheduledAttentionCount}
            </span>
          )}
        </button>
      </div>

      <div className="left-sidebar__scroll">
        {hasPinnedItems && (
          <section
            className="left-sidebar__section left-sidebar__pinned"
            aria-labelledby="pinned-heading"
          >
            <h2
              id="pinned-heading"
              className="left-sidebar__section-title left-sidebar__standalone-title"
            >
              {t('sidebar.pinned')}
            </h2>
            <div className="left-sidebar__pinned-list">
              {pinnedRootConversations.map((conversation) => renderConversationRow(conversation))}
              {pinnedProjects.map((project) => renderProjectGroup(project, true, []))}
            </div>
          </section>
        )}

        {uiPreferences.sidebarSectionOrder === 'conversations_first' ? (
          <>
            {conversationsSection}
            {projectsSection}
          </>
        ) : (
          <>
            {projectsSection}
            {conversationsSection}
          </>
        )}
      </div>

      <LeftSidebarAccountFooter
        onOpenSettings={onOpenSettings}
        t={t}
        uiPreferences={uiPreferences}
      />

      <LeftSidebarDialogs
        archiveProjectCount={archiveProjectCount}
        bulkArchiveCount={bulkArchiveCount}
        conversationRenameValue={conversationRenameValue}
        onArchiveAllProjectConversations={onArchiveAllProjectConversations}
        onArchiveAllRootConversations={onArchiveAllRootConversations}
        onArchiveProjectConversations={onArchiveProjectConversations}
        onCancelArchiveProject={() => setPendingArchiveProject(null)}
        onCancelBulkArchive={() => setPendingBulkArchiveScope(null)}
        onCancelConversationRename={() => setRenamingConversation(null)}
        onCancelRemoveProject={() => setPendingRemoveProject(null)}
        onConfirmConversationRename={confirmRenameConversation}
        onConversationRenameValueChange={setConversationRenameValue}
        onRemoveProject={onRemoveProject}
        pendingArchiveProject={pendingArchiveProject}
        pendingBulkArchiveScope={pendingBulkArchiveScope}
        pendingRemoveProject={pendingRemoveProject}
        renamingConversation={renamingConversation}
        t={t}
      />

      {isSearchDialogOpen && (
        <LeftSidebarSearchDialog
          conversations={visibleConversations}
          labels={{
            chats: t('sidebar.searchChats'),
            empty: t('sidebar.searchEmpty'),
            inputPlaceholder: t('sidebar.searchPlaceholder'),
            noMatches: t('sidebar.searchNoMatches'),
            noProject: t('archive.noProject'),
            searchFailed: t('sidebar.searchFailed'),
            searching: t('sidebar.searchingChats')
          }}
          onClose={() => setIsSearchDialogOpen(false)}
          onSelectConversation={onSelectConversation}
          projects={projects}
        />
      )}
    </aside>
  )
})
