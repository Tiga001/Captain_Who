import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode, Ref } from 'react'
import type {
  GitReviewBranch,
  GitReviewCommit,
  GitReviewFile,
  GitReviewTarget
} from '@mycopilot/protocol'
import {
  AlertCircle,
  Check,
  Ellipsis,
  FileSearch,
  FileText,
  Files,
  FolderOpen,
  ListCollapse,
  LoaderCircle,
  RefreshCw,
  Search,
  WrapText,
  X
} from 'lucide-react'
import { projectWorkspaceRevision, type AppProject } from '../../config/projectConfig'
import { GitReviewRepositorySelector } from './GitReviewRepositorySelector'
import { useGitRepositorySources } from './useGitRepositorySources'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { dismissActiveTooltip, Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewDiffCard } from './GitReviewDiffCard'
import { GitReviewContextRow } from './GitReviewContextRow'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import { GitReviewMenuPortal } from './GitReviewMenuPortal'
import { GitReviewSourceSelector } from './GitReviewSourceSelector'
import { canHydrateGitReviewFile } from './gitReviewFileCapabilities'
import { copyGitReviewFilePath } from './gitReviewClient'
import { loadGitReviewPreferences, saveGitReviewPreferences } from './gitReviewPreferences'
import { projectGitReviewSummary } from './gitReviewSummaryProjection'
import {
  getGitReviewTargetCapabilities,
  type GitReviewTargetCapabilities
} from './gitReviewTargetCapabilities'
import { getTargetGitReviewViewMode } from './gitReviewViewMode'
import type { GitReviewViewMode } from './gitReviewViewMode'
import { gitReviewTargetKey, useGitReview } from './useGitReview'
import { useGitReviewRepositoryContext } from './useGitReviewRepositoryContext'
import './GitReviewPanel.css'

interface GitReviewPanelProps {
  conversationId?: string | null
  isActive: boolean
  onOpenFile: (path: string, folderId?: string, assistantMessageId?: string) => void
  project?: AppProject
  projectId: string
  targetNavigation?: {
    filePath?: string
    requestId: number
    target: GitReviewTarget
  }
}

type OpenMenu = 'source' | 'repository' | 'options' | null

interface PendingFileAlignment {
  fileId: string
  token: number
}

interface PendingReviewFileNavigation {
  filePath: string
  requestId: number
  targetKey: string
}

interface GitReviewFileVisibility {
  near: Set<string>
  visible: Set<string>
}

function emptyFileVisibility(): GitReviewFileVisibility {
  return { near: new Set(), visible: new Set() }
}

function normalizeReviewFilePath(path: string): string {
  const normalized = path.trim().replace(/\\/g, '/')
  return normalized.startsWith('./@workspace/') ? normalized : normalized.replace(/^\.\/+/, '')
}

function findReviewFileByPath(
  files: readonly GitReviewFile[],
  requestedPath: string
): GitReviewFile | undefined {
  const normalizedRequestedPath = normalizeReviewFilePath(requestedPath)
  if (!normalizedRequestedPath) return undefined

  return files.find(
    (file) =>
      normalizeReviewFilePath(file.workspacePath ?? file.path) === normalizedRequestedPath ||
      (file.previousPath !== undefined &&
        normalizeReviewFilePath(file.previousPath) === normalizedRequestedPath)
  )
}

export function GitReviewPanel({
  conversationId,
  isActive,
  onOpenFile,
  project,
  projectId,
  targetNavigation
}: GitReviewPanelProps): ReactNode {
  const { language, t } = useFrontendConfig()
  const [reviewPreferences, setReviewPreferences] = useState(loadGitReviewPreferences)
  const projectRevision = project ? projectWorkspaceRevision(project) : ''
  const multiFolder = (project?.folders.length ?? 0) > 1
  const sourceInspection = useGitRepositorySources(projectId, projectRevision, multiFolder)
  const sourceFolders = useMemo(
    () => sourceInspection?.inspection?.folders?.filter((folder) => folder.state === 'ready') ?? [],
    [sourceInspection]
  )
  const [rememberedFolder, setRememberedFolder] = useState<{
    projectId: string
    folderId: string
  } | null>(null)
  const [allLastTurn, setAllLastTurn] = useState(true)
  const selectedFolderId = multiFolder
    ? (sourceFolders.find(
        (folder) =>
          rememberedFolder?.projectId === projectId && folder.folderId === rememberedFolder.folderId
      )?.folderId ?? sourceInspection?.inspection?.defaultFolderId)
    : undefined
  const {
    cancelQueuedFileContentsExcept,
    cancelQueuedFileDiffsExcept,
    diffStates,
    dismissMutationError,
    fileContentStates,
    loadFileDiff,
    loadFileContent,
    mutateFile,
    mutationError,
    pendingFileId,
    refresh,
    retryFileDiff,
    target,
    setHotDiffFileIds,
    setHotFullContentFileIds,
    setTarget,
    summaryState: sourceSummaryState,
    sourceKey
  } = useGitReview(
    projectId,
    isActive && (!multiFolder || Boolean(sourceInspection?.inspection)),
    targetNavigation?.target,
    {
      folderId: selectedFolderId,
      allLastTurn: multiFolder && allLastTurn,
      revision: projectRevision
    }
  )
  const { load: loadRepositoryContext, state: repositoryState } = useGitReviewRepositoryContext(
    projectId,
    selectedFolderId,
    projectRevision
  )
  const targetKey = gitReviewTargetKey(target)
  const repositoryBinding = JSON.stringify([projectId, selectedFolderId, projectRevision])
  const previousRepositoryBinding = useRef(repositoryBinding)
  useLayoutEffect(() => {
    const changed = previousRepositoryBinding.current !== repositoryBinding
    previousRepositoryBinding.current = repositoryBinding
    if (changed && (target.kind === 'commit' || target.kind === 'branch')) {
      setTarget({ kind: 'uncommitted' })
    }
  }, [repositoryBinding, setTarget, target.kind])
  const targetKind = target.kind
  const capabilities = getGitReviewTargetCapabilities(targetKind)
  const projectedSummary = useMemo(() => {
    if (!sourceSummaryState.value) return undefined
    return projectGitReviewSummary(sourceSummaryState.value, {
      includeFilesWithoutStats: reviewPreferences.showAllFileTypes
    })
  }, [reviewPreferences.showAllFileTypes, sourceSummaryState.value])
  const summaryError = sourceSummaryState.status === 'error' ? sourceSummaryState.error : ''
  const summaryState = useMemo((): typeof sourceSummaryState => {
    if (sourceSummaryState.status === 'ready') {
      return { status: 'ready', value: projectedSummary ?? sourceSummaryState.value }
    }
    if (sourceSummaryState.status === 'error') {
      return {
        error: summaryError,
        status: 'error',
        ...(projectedSummary ? { value: projectedSummary } : {})
      }
    }
    return {
      status: sourceSummaryState.status,
      ...(projectedSummary ? { value: projectedSummary } : {})
    }
  }, [projectedSummary, sourceSummaryState.status, sourceSummaryState.value, summaryError])
  const handledTargetNavigationRequestRef = useRef<number | null>(null)
  const [expandedFileIds, setExpandedFileIds] = useState<Set<string>>(() => new Set())
  const [selectedFileId, setSelectedFileId] = useState<string | null>(null)
  const [pendingReviewFileNavigation, setPendingReviewFileNavigation] =
    useState<PendingReviewFileNavigation | null>(null)
  const [viewMode, setViewMode] = useState<GitReviewViewMode>('unified')
  const [wrapLines, setWrapLines] = useState(false)
  const [showFileList, setShowFileList] = useState(false)
  const [showSearch, setShowSearch] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null)
  const [commitPreview, setCommitPreview] = useState<GitReviewCommit | undefined>()
  const [rememberedBranchBaseRef, setRememberedBranchBaseRef] = useState<string | undefined>()
  const [restoreCandidate, setRestoreCandidate] = useState<GitReviewFile | null>(null)
  const [fileVisibility, setFileVisibility] = useState<GitReviewFileVisibility>(emptyFileVisibility)
  const [diffLayoutWidth, setDiffLayoutWidth] = useState(0)
  const contentRef = useRef<HTMLDivElement>(null)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const optionsButtonRef = useRef<HTMLButtonElement>(null)
  const fileElementsRef = useRef(new Map<string, HTMLElement>())
  const diffStatesRef = useRef(diffStates)
  const initializedQueryRef = useRef<string | null>(null)
  const pendingFileAlignmentRef = useRef<PendingFileAlignment | null>(null)
  const fileAlignmentSequenceRef = useRef(0)
  const files = useMemo(() => summaryState.value?.files ?? [], [summaryState.value])
  const repositoryDefaultBaseRef = useMemo(() => {
    if (repositoryState.status !== 'ready') return undefined
    return (
      repositoryState.value.defaultBaseRef ??
      repositoryState.value.branches.find((branch) => branch.isDefault)?.ref ??
      repositoryState.value.branches[0]?.ref
    )
  }, [repositoryState])
  const branchBaseRef =
    target.kind === 'branch'
      ? target.baseRef
      : (rememberedBranchBaseRef ?? repositoryDefaultBaseRef)
  const matchingSummary =
    summaryState.value && gitReviewTargetKey(summaryState.value.target) === targetKey
      ? summaryState.value
      : undefined
  const hasContextRow = targetKind === 'commit' || targetKind === 'branch'

  const cancelPendingFileAlignment = useCallback(() => {
    pendingFileAlignmentRef.current = null
    contentRef.current?.scrollBy({ behavior: 'auto', left: 0, top: 0 })
  }, [])

  useLayoutEffect(() => {
    if (
      !targetNavigation ||
      handledTargetNavigationRequestRef.current === targetNavigation.requestId
    ) {
      return
    }
    handledTargetNavigationRequestRef.current = targetNavigation.requestId
    const filePath = targetNavigation.filePath?.trim()
    setPendingReviewFileNavigation(
      filePath
        ? {
            filePath,
            requestId: targetNavigation.requestId,
            targetKey: gitReviewTargetKey(targetNavigation.target)
          }
        : null
    )
    if (targetKey !== gitReviewTargetKey(targetNavigation.target)) {
      setTarget(targetNavigation.target)
    }
  }, [setTarget, targetKey, targetNavigation])

  useEffect(() => {
    if (target.kind === 'lastTurn' && conversationId && target.conversationId !== conversationId) {
      setTarget({ kind: 'lastTurn', conversationId })
    }
  }, [conversationId, setTarget, target])

  useEffect(() => {
    diffStatesRef.current = diffStates
  }, [diffStates])

  useEffect(() => {
    if (!isActive) {
      setOpenMenu(null)
      dismissActiveTooltip()
      cancelPendingFileAlignment()
    }
  }, [cancelPendingFileAlignment, isActive])

  useEffect(() => {
    setRestoreCandidate(null)
    setExpandedFileIds(new Set())
    setSelectedFileId(null)
    setSearchQuery('')
    setFileVisibility(emptyFileVisibility())
    cancelPendingFileAlignment()
  }, [cancelPendingFileAlignment, projectId, targetKey, sourceKey])

  useEffect(() => {
    setCommitPreview(undefined)
    setRememberedBranchBaseRef(undefined)
  }, [projectId, selectedFolderId, projectRevision])

  useEffect(() => {
    if (target.kind === 'branch') void loadRepositoryContext()
  }, [loadRepositoryContext, target.kind])

  useEffect(() => {
    if (!openMenu) return
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target
      if (!(target instanceof Element)) return
      if (
        target.closest('.git-review__menu') ||
        target.closest('.git-review__source-control') ||
        target.closest('.git-review__repository-control') ||
        target.closest('.git-review__options-control')
      ) {
        return
      }
      setOpenMenu(null)
    }
    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [openMenu])

  useEffect(() => {
    if (showSearch) searchInputRef.current?.focus()
  }, [showSearch])

  useEffect(() => {
    const content = contentRef.current
    if (!content) return undefined
    const measure = (): void => {
      const width = Math.max(0, Math.round(content.getBoundingClientRect().width))
      setDiffLayoutWidth((current) => (current === width ? current : width))
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(measure)
    observer.observe(content)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    if (!summaryState.value || gitReviewTargetKey(summaryState.value.target) !== targetKey) return
    const fileIds = new Set(files.map((file) => file.id))
    const queryKey = `${projectId}:${targetKey}`
    const initializeQuery = initializedQueryRef.current !== queryKey
    initializedQueryRef.current = queryKey

    setExpandedFileIds((current) => {
      if (initializeQuery) return new Set()
      const next = new Set([...current].filter((fileId) => fileIds.has(fileId)))
      return setsEqual(current, next) ? current : next
    })
    setSelectedFileId((current) => {
      if (current && fileIds.has(current)) return current
      return files[0]?.id ?? null
    })
  }, [files, projectId, summaryState.value, targetKey])

  const normalizedQuery = searchQuery.trim().toLocaleLowerCase()
  const filteredFiles = useMemo(() => {
    if (!normalizedQuery) return files
    return files.filter((file) =>
      `${file.previousPath ?? ''}\n${file.path}`.toLocaleLowerCase().includes(normalizedQuery)
    )
  }, [files, normalizedQuery])

  useEffect(() => {
    setFileVisibility(emptyFileVisibility())
    if (!isActive) return undefined
    const root = contentRef.current
    if (!root) return undefined

    if (typeof IntersectionObserver === 'undefined') {
      const allFileIds = new Set(filteredFiles.map((file) => file.id))
      setFileVisibility({ near: allFileIds, visible: new Set(allFileIds) })
      return undefined
    }

    let disposed = false
    let updateFrame: number | null = null
    const pendingUpdates: Record<keyof GitReviewFileVisibility, Map<string, boolean>> = {
      near: new Map(),
      visible: new Map()
    }
    const flushVisibilityUpdates = (): void => {
      updateFrame = null
      if (disposed) return
      const updates = {
        near: new Map(pendingUpdates.near),
        visible: new Map(pendingUpdates.visible)
      }
      pendingUpdates.near.clear()
      pendingUpdates.visible.clear()
      setFileVisibility((current) => {
        let next: GitReviewFileVisibility | null = null
        for (const field of ['near', 'visible'] as const) {
          if (updates[field].size === 0) continue
          const values = new Set((next ?? current)[field])
          for (const [fileId, included] of updates[field]) {
            updateMembership(values, fileId, included)
          }
          if (!setsEqual((next ?? current)[field], values)) {
            next = { ...(next ?? current), [field]: values }
          }
        }
        return next ?? current
      })
    }
    const scheduleVisibilityFlush = (): void => {
      if (updateFrame === null) updateFrame = requestAnimationFrame(flushVisibilityUpdates)
    }
    const createObserver = (
      field: keyof GitReviewFileVisibility,
      rootMargin: string
    ): IntersectionObserver =>
      new IntersectionObserver(
        (entries) => {
          if (disposed) return
          for (const entry of entries) {
            const fileId = (entry.target as HTMLElement).dataset.reviewFileId
            if (!fileId) continue
            pendingUpdates[field].set(
              fileId,
              entry.isIntersecting &&
                entry.intersectionRect.width > 0 &&
                entry.intersectionRect.height > 0
            )
          }
          scheduleVisibilityFlush()
        },
        { root, rootMargin, threshold: [0, 0.001] }
      )

    const nearObserver = createObserver('near', '640px 0px')
    const visibleObserver = createObserver('visible', '0px')
    for (const file of filteredFiles) {
      const element = fileElementsRef.current.get(file.id)
      if (element) {
        nearObserver.observe(element)
        visibleObserver.observe(element)
      }
    }
    return () => {
      disposed = true
      nearObserver.disconnect()
      visibleObserver.disconnect()
      if (updateFrame !== null) cancelAnimationFrame(updateFrame)
      pendingUpdates.near.clear()
      pendingUpdates.visible.clear()
    }
  }, [filteredFiles, isActive])

  const demandedDiffFileIds = useMemo(() => {
    const demanded = new Set<string>()
    if (!isActive) return demanded
    for (const file of filteredFiles) {
      if (
        expandedFileIds.has(file.id) &&
        (fileVisibility.near.has(file.id) || selectedFileId === file.id)
      ) {
        demanded.add(file.id)
      }
    }
    return demanded
  }, [expandedFileIds, fileVisibility.near, filteredFiles, isActive, selectedFileId])

  const demandedFullContentFileIds = useMemo(() => {
    const demanded = new Set<string>()
    if (!isActive || !reviewPreferences.loadFullFiles) return demanded
    for (const file of filteredFiles) {
      const diffState = diffStates[file.id]
      if (
        expandedFileIds.has(file.id) &&
        fileVisibility.visible.has(file.id) &&
        canHydrateGitReviewFile(file.status) &&
        diffState?.status === 'ready' &&
        diffState.value.status === 'ready'
      ) {
        demanded.add(file.id)
      }
    }
    return demanded
  }, [
    diffStates,
    expandedFileIds,
    fileVisibility.visible,
    filteredFiles,
    isActive,
    reviewPreferences.loadFullFiles
  ])

  useEffect(() => {
    setHotDiffFileIds(demandedDiffFileIds)
    cancelQueuedFileDiffsExcept(demandedDiffFileIds)
    if (!isActive) return

    if (selectedFileId && demandedDiffFileIds.has(selectedFileId)) {
      loadFileDiff(selectedFileId, 'high')
    }
    for (const fileId of demandedDiffFileIds) {
      if (fileId !== selectedFileId) loadFileDiff(fileId, 'normal')
    }
    // Keep the selected ready payload at the hot end of the LRU after batch completions.
    if (selectedFileId && demandedDiffFileIds.has(selectedFileId)) {
      loadFileDiff(selectedFileId, 'high')
    }
  }, [
    cancelQueuedFileDiffsExcept,
    demandedDiffFileIds,
    diffStates,
    isActive,
    loadFileDiff,
    selectedFileId,
    setHotDiffFileIds
  ])

  useEffect(() => {
    setHotFullContentFileIds(demandedFullContentFileIds)
    cancelQueuedFileContentsExcept(demandedFullContentFileIds)
    if (!isActive) return
    if (selectedFileId && demandedFullContentFileIds.has(selectedFileId)) {
      loadFileContent(selectedFileId)
    }
    for (const fileId of demandedFullContentFileIds) {
      if (fileId !== selectedFileId) loadFileContent(fileId)
    }
  }, [
    cancelQueuedFileContentsExcept,
    demandedFullContentFileIds,
    fileContentStates,
    isActive,
    loadFileContent,
    selectedFileId,
    setHotFullContentFileIds
  ])

  const allExpanded = files.length > 0 && files.every((file) => expandedFileIds.has(file.id))

  const toggleFile = useCallback(
    (fileId: string) => {
      cancelPendingFileAlignment()
      setExpandedFileIds((current) => {
        const next = new Set(current)
        if (next.has(fileId)) next.delete(fileId)
        else next.add(fileId)
        return next
      })
      setSelectedFileId(fileId)
    },
    [cancelPendingFileAlignment]
  )

  const toggleAllFiles = useCallback(() => {
    cancelPendingFileAlignment()
    setExpandedFileIds((current) =>
      files.length > 0 && files.every((file) => current.has(file.id))
        ? new Set()
        : new Set(files.map((file) => file.id))
    )
  }, [cancelPendingFileAlignment, files])

  const alignFileHeader = useCallback((fileId: string, behavior: ScrollBehavior): boolean => {
    const content = contentRef.current
    const target = fileElementsRef.current.get(fileId)
    if (!content || !target) return false
    const contentRect = content.getBoundingClientRect()
    const targetRect = target.getBoundingClientRect()
    content.scrollTo({
      behavior,
      top: content.scrollTop + targetRect.top - contentRect.top
    })
    return true
  }, [])

  const selectFile = useCallback(
    (fileId: string) => {
      const token = fileAlignmentSequenceRef.current + 1
      fileAlignmentSequenceRef.current = token
      pendingFileAlignmentRef.current = { fileId, token }
      setSelectedFileId(fileId)
      setExpandedFileIds((current) => {
        if (current.has(fileId)) return current
        const next = new Set(current)
        next.add(fileId)
        return next
      })
      requestAnimationFrame(() => {
        const pending = pendingFileAlignmentRef.current
        if (!pending || pending.token !== token) return
        alignFileHeader(fileId, 'smooth')
        const state = diffStatesRef.current[fileId]
        if (state?.status === 'ready' || state?.status === 'error') {
          pendingFileAlignmentRef.current = null
        }
      })
    },
    [alignFileHeader]
  )

  useEffect(() => {
    const pending = pendingReviewFileNavigation
    if (
      !pending ||
      !isActive ||
      summaryState.status !== 'ready' ||
      gitReviewTargetKey(summaryState.value.target) !== pending.targetKey
    ) {
      return
    }

    // The last-turn summary is the sole authority for whether a chat-card path can be focused.
    // Missing paths deliberately fall back to the ordinary review-page state.
    const targetFile = findReviewFileByPath(summaryState.value.files, pending.filePath)
    setPendingReviewFileNavigation((current) =>
      current?.requestId === pending.requestId ? null : current
    )
    if (!targetFile) return

    setSearchQuery('')
    selectFile(targetFile.id)
  }, [isActive, pendingReviewFileNavigation, selectFile, summaryState])

  const handleMutateFile = useCallback(
    (fileId: string, action: Parameters<typeof mutateFile>[1]) => {
      void mutateFile(fileId, action).catch(() => undefined)
    },
    [mutateFile]
  )

  const historicalAssistantMessageId = matchingSummary?.assistantMessageId
  const handleCopyFile = useCallback(
    (path: string, file: GitReviewFile) =>
      copyGitReviewFilePath({
        projectId,
        path,
        ...(file.sourceFolderId ? { folderId: file.sourceFolderId } : {}),
        ...(historicalAssistantMessageId
          ? { assistantMessageId: historicalAssistantMessageId }
          : {})
      }),
    [projectId, historicalAssistantMessageId]
  )
  const handleOpenFile = useCallback(
    (path: string, file: GitReviewFile) => {
      if (file.sourceFolderId || historicalAssistantMessageId) {
        onOpenFile(path, file.sourceFolderId, historicalAssistantMessageId)
      } else onOpenFile(path)
    },
    [onOpenFile, historicalAssistantMessageId]
  )
  const handleRepositorySelect = useCallback(
    (folderId: string | null) => {
      if (folderId === null) {
        setAllLastTurn(true)
        return
      }
      setRememberedFolder({ projectId, folderId })
      if (target.kind === 'lastTurn') setAllLastTurn(false)
      // A commit SHA or branch from another repository must never be replayed against this one.
      if (target.kind === 'commit' || target.kind === 'branch') setTarget({ kind: 'uncommitted' })
      setPendingReviewFileNavigation(null)
    },
    [projectId, setTarget, target.kind]
  )

  useEffect(() => {
    const pending = pendingFileAlignmentRef.current
    if (!pending) return undefined
    const state = diffStates[pending.fileId]
    if (state?.status !== 'ready' && state?.status !== 'error') return undefined

    const animationFrame = requestAnimationFrame(() => {
      const current = pendingFileAlignmentRef.current
      if (!current || current.token !== pending.token) return
      alignFileHeader(current.fileId, 'auto')
      pendingFileAlignmentRef.current = null
    })
    return () => cancelAnimationFrame(animationFrame)
  }, [alignFileHeader, diffStates])

  const handleTargetChange = useCallback(
    (nextTarget: GitReviewTarget) => {
      setPendingReviewFileNavigation(null)
      if (gitReviewTargetKey(nextTarget) !== targetKey) setTarget(nextTarget)
      setOpenMenu(null)
    },
    [setTarget, targetKey]
  )

  const handleCommitSelect = useCallback(
    (commit: GitReviewCommit) => {
      setCommitPreview(commit)
      handleTargetChange({ kind: 'commit', commitSha: commit.sha })
    },
    [handleTargetChange]
  )

  const handleBranchSourceSelect = useCallback(() => {
    if (!branchBaseRef) return
    setRememberedBranchBaseRef(branchBaseRef)
    handleTargetChange({ kind: 'branch', baseRef: branchBaseRef })
  }, [branchBaseRef, handleTargetChange])

  const handleBranchSelect = useCallback(
    (branch: GitReviewBranch) => {
      setRememberedBranchBaseRef(branch.ref)
      handleTargetChange({ kind: 'branch', baseRef: branch.ref })
    },
    [handleTargetChange]
  )

  const closeOptionsMenu = useCallback(() => {
    setOpenMenu(null)
    window.requestAnimationFrame(() => optionsButtonRef.current?.focus())
  }, [])

  const toggleSearch = useCallback(() => {
    setShowSearch((current) => {
      if (current) setSearchQuery('')
      return !current
    })
    setOpenMenu(null)
  }, [])

  const stats = matchingSummary?.stats ?? null
  const statsLabel = stats
    ? formatTranslation(
        t,
        stats.lineCountsComplete ? 'gitReview.stats.summary' : 'gitReview.stats.incomplete',
        {
          additions: stats.additions,
          deletions: stats.deletions,
          files: stats.fileCount
        }
      )
    : null
  const targetViewMode = getTargetGitReviewViewMode(viewMode)
  const nextViewLabel =
    targetViewMode === 'split' ? t('gitReview.view.switchSplit') : t('gitReview.view.switchUnified')

  return (
    <div className="git-review">
      <header className="git-review__toolbar" data-has-context={hasContextRow ? 'true' : undefined}>
        <div className="git-review__toolbar-summary">
          {multiFolder && (
            <GitReviewRepositorySelector
              folders={sourceFolders}
              selectedFolderId={selectedFolderId}
              allSelected={targetKind === 'lastTurn' && allLastTurn}
              allowAll={targetKind === 'lastTurn'}
              isOpen={openMenu === 'repository'}
              onOpenChange={(open) => setOpenMenu(open ? 'repository' : null)}
              onSelect={handleRepositorySelect}
              t={t}
            />
          )}
          <GitReviewSourceSelector
            branchBaseRef={branchBaseRef}
            conversationId={conversationId}
            fileCount={stats?.fileCount}
            isOpen={openMenu === 'source'}
            language={language}
            onOpenChange={(open) => setOpenMenu(open ? 'source' : null)}
            onRequestRepositoryContext={(force) => void loadRepositoryContext(force)}
            onSelectBranch={handleBranchSourceSelect}
            onSelectCommit={handleCommitSelect}
            onSelectTarget={handleTargetChange}
            projectId={projectId}
            folderId={selectedFolderId}
            revision={projectRevision}
            repositoryState={repositoryState}
            t={t}
            target={target}
          />
          {stats && statsLabel && (
            <div className="git-review__line-stats" aria-label={statsLabel}>
              <span className="git-review__line-stats-addition" aria-hidden="true">
                +{stats.lineCountsComplete ? stats.additions : '?'}
              </span>
              <span className="git-review__line-stats-deletion" aria-hidden="true">
                -{stats.lineCountsComplete ? stats.deletions : '?'}
              </span>
            </div>
          )}
        </div>

        <div className="git-review__toolbar-actions">
          <div className="git-review__options-control">
            <ToolbarButton
              active={openMenu === 'options'}
              ariaExpanded={openMenu === 'options'}
              ariaHasPopup="menu"
              buttonRef={optionsButtonRef}
              label={t('gitReview.options')}
              onClick={() => setOpenMenu((current) => (current === 'options' ? null : 'options'))}
            >
              <Ellipsis aria-hidden="true" />
            </ToolbarButton>
            {openMenu === 'options' && (
              <GitReviewMenuPortal
                anchorRef={optionsButtonRef}
                ariaLabel={t('gitReview.options')}
                autoFocus="first"
                className="git-review__options-menu"
                onEscape={closeOptionsMenu}
                placement="bottom-end"
              >
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    closeOptionsMenu()
                    void refresh()
                  }}
                >
                  <RefreshCw aria-hidden="true" />
                  {t('gitReview.refresh')}
                </button>
                <button
                  type="button"
                  role="menuitemcheckbox"
                  aria-checked={wrapLines}
                  onClick={() => {
                    setWrapLines((current) => !current)
                    closeOptionsMenu()
                  }}
                >
                  <WrapText aria-hidden="true" />
                  {t('gitReview.wrapLines')}
                  {wrapLines && <Check className="git-review__menu-check" aria-hidden="true" />}
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setReviewPreferences((current) => {
                      const next = {
                        ...current,
                        showAllFileTypes: !current.showAllFileTypes
                      }
                      saveGitReviewPreferences(next)
                      return next
                    })
                    closeOptionsMenu()
                  }}
                >
                  <Files aria-hidden="true" />
                  {reviewPreferences.showAllFileTypes
                    ? t('gitReview.dontShowAllFileTypes')
                    : t('gitReview.showAllFileTypes')}
                </button>
                <div className="git-review__menu-separator" role="separator" />
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setReviewPreferences((current) => {
                      const next = { ...current, loadFullFiles: !current.loadFullFiles }
                      saveGitReviewPreferences(next)
                      return next
                    })
                    closeOptionsMenu()
                  }}
                >
                  <FileText aria-hidden="true" />
                  {reviewPreferences.loadFullFiles
                    ? t('gitReview.dontLoadFullFiles')
                    : t('gitReview.loadFullFiles')}
                </button>
              </GitReviewMenuPortal>
            )}
          </div>

          <ToolbarButton
            active={allExpanded}
            disabled={files.length === 0}
            label={allExpanded ? t('gitReview.collapseAll') : t('gitReview.expandAll')}
            onClick={toggleAllFiles}
          >
            <ListCollapse aria-hidden="true" />
          </ToolbarButton>
          <ToolbarButton active={showSearch} label={t('gitReview.search')} onClick={toggleSearch}>
            <FileSearch aria-hidden="true" />
          </ToolbarButton>
          <ToolbarButton
            label={nextViewLabel}
            onClick={() => setViewMode((current) => getTargetGitReviewViewMode(current))}
          >
            <DiffLayoutIcon targetMode={targetViewMode} />
          </ToolbarButton>
          <ToolbarButton
            active={showFileList}
            label={showFileList ? t('gitReview.files.hide') : t('gitReview.files.show')}
            onClick={() => setShowFileList((current) => !current)}
          >
            <FolderOpen aria-hidden="true" />
          </ToolbarButton>
        </div>

        {hasContextRow && (
          <GitReviewContextRow
            commitPreview={commitPreview}
            isActive={isActive}
            language={language}
            onRetryBranches={() => void loadRepositoryContext(true)}
            onSelectBranch={handleBranchSelect}
            repositoryState={repositoryState}
            summaryContext={matchingSummary?.context}
            t={t}
            target={target}
          />
        )}
      </header>

      {showSearch && (
        <div className="git-review__search-bar">
          <Search aria-hidden="true" />
          <input
            ref={searchInputRef}
            type="search"
            value={searchQuery}
            aria-label={t('gitReview.search')}
            placeholder={t('gitReview.searchPlaceholder')}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') toggleSearch()
              if (event.key === 'Enter' && filteredFiles[0]) selectFile(filteredFiles[0].id)
            }}
          />
          <button type="button" aria-label={t('gitReview.search.close')} onClick={toggleSearch}>
            <X aria-hidden="true" />
          </button>
        </div>
      )}

      {summaryState.status === 'loading' && summaryState.value && (
        <div className="git-review__refreshing" role="status">
          <LoaderCircle className="git-review__spinner" aria-hidden="true" />
          {t('gitReview.refreshing')}
        </div>
      )}

      {summaryState.status === 'error' && summaryState.value && (
        <div className="git-review__inline-error" role="alert">
          <AlertCircle aria-hidden="true" />
          <span>{summaryState.error}</span>
          <button type="button" onClick={() => void refresh()}>
            {t('gitReview.retry')}
          </button>
        </div>
      )}

      {sourceInspection?.error && (
        <div className="git-review__inline-error" role="alert">
          {sourceInspection.error}
        </div>
      )}
      {summaryState.value?.message && (
        <div className="git-review__truncated" role="status">
          {summaryState.value.message}
        </div>
      )}
      {mutationError && (
        <div className="git-review__inline-error" role="alert">
          <AlertCircle aria-hidden="true" />
          <span>{mutationError}</span>
          <button
            type="button"
            aria-label={t('gitReview.error.dismiss')}
            onClick={dismissMutationError}
          >
            <X aria-hidden="true" />
          </button>
        </div>
      )}

      {summaryState.value?.truncated && (
        <div className="git-review__truncated" role="status">
          {t('gitReview.truncated')}
        </div>
      )}

      <main className="git-review__body" data-file-list={showFileList ? 'visible' : 'hidden'}>
        {showFileList && summaryState.value && files.length > 0 && (
          <FileList
            files={filteredFiles}
            groupSources={matchingSummary?.source?.kind === 'all'}
            onSelect={selectFile}
            selectedFileId={selectedFileId}
            t={t}
          />
        )}

        <div
          className="git-review__content"
          ref={contentRef}
          onKeyDownCapture={(event) => {
            if (isManualScrollKey(event.key)) cancelPendingFileAlignment()
          }}
          onPointerDownCapture={cancelPendingFileAlignment}
          onTouchStartCapture={cancelPendingFileAlignment}
          onWheelCapture={cancelPendingFileAlignment}
        >
          <GitReviewContent
            capabilities={capabilities}
            diffStates={diffStates}
            diffLayoutWidth={diffLayoutWidth}
            expandedFileIds={expandedFileIds}
            fileElementsRef={fileElementsRef}
            fileContentStates={fileContentStates}
            filteredFiles={filteredFiles}
            hasSearchQuery={Boolean(normalizedQuery)}
            isActive={isActive}
            loadFullFiles={reviewPreferences.loadFullFiles}
            mutateFile={handleMutateFile}
            pendingFileId={pendingFileId}
            onCopyFile={handleCopyFile}
            onOpenFile={handleOpenFile}
            onRestore={setRestoreCandidate}
            nearFileIds={fileVisibility.near}
            targetKind={targetKind}
            scrollRootRef={contentRef}
            selectedFileId={selectedFileId}
            retryFileDiff={retryFileDiff}
            reviewSnapshotId={summaryState.value?.snapshotId}
            summaryState={summaryState}
            t={t}
            toggleFile={toggleFile}
            viewMode={viewMode}
            visibleFileIds={fileVisibility.visible}
            wrapLines={wrapLines}
            onRefresh={refresh}
          />
        </div>
      </main>

      {restoreCandidate && (
        <ConfirmationDialog
          cancelLabel={t('gitReview.restore.cancel')}
          confirmLabel={t('gitReview.restore.confirm')}
          description={formatTranslation(
            t,
            restoreCandidate.status === 'untracked'
              ? 'gitReview.restore.untrackedDescription'
              : 'gitReview.restore.trackedDescription',
            { path: restoreCandidate.path }
          )}
          onCancel={() => setRestoreCandidate(null)}
          onConfirm={async () => {
            try {
              await mutateFile(restoreCandidate.id, 'restore')
            } finally {
              setRestoreCandidate(null)
            }
          }}
          title={t('gitReview.restore.title')}
        />
      )}
    </div>
  )
}

interface ToolbarButtonProps {
  active?: boolean
  ariaExpanded?: boolean
  ariaHasPopup?: 'menu'
  buttonRef?: Ref<HTMLButtonElement>
  children: ReactNode
  disabled?: boolean
  label: string
  onClick: () => void
}

function ToolbarButton({
  active,
  ariaExpanded,
  ariaHasPopup,
  buttonRef,
  children,
  disabled,
  label,
  onClick
}: ToolbarButtonProps) {
  return (
    <Tooltip
      anchorClassName="git-review__toolbar-button-wrap"
      content={label}
      preferredPlacement="bottom"
    >
      <button
        className="git-review__toolbar-button"
        type="button"
        aria-expanded={ariaExpanded}
        aria-haspopup={ariaHasPopup}
        aria-label={label}
        aria-pressed={active === undefined ? undefined : active}
        data-active={active ? 'true' : undefined}
        disabled={disabled}
        onClick={onClick}
        ref={buttonRef}
      >
        {children}
      </button>
    </Tooltip>
  )
}

function DiffLayoutIcon({ targetMode }: { targetMode: GitReviewViewMode }): ReactNode {
  return (
    <span className="git-review__layout-icon" data-mode={targetMode} aria-hidden="true">
      <span />
      <span />
    </span>
  )
}

interface FileListProps {
  groupSources?: boolean
  files: GitReviewFile[]
  onSelect: (fileId: string) => void
  selectedFileId: string | null
  t: ReturnType<typeof useFrontendConfig>['t']
}

function FileList({ files, groupSources, onSelect, selectedFileId, t }: FileListProps): ReactNode {
  return (
    <aside className="git-review__file-list" aria-label={t('gitReview.fileList')}>
      <div className="git-review__file-list-heading">
        <span>{t('gitReview.fileList')}</span>
        <span>{files.length}</span>
      </div>
      <div className="git-review__file-list-scroll">
        {files.map((file, index) => {
          const pathParts = file.path.split('/')
          const name = pathParts.pop() || file.path
          const directory = pathParts.join('/')
          return (
            <Fragment key={file.id}>
              {groupSources &&
                (index === 0 || files[index - 1].sourceFolderId !== file.sourceFolderId) && (
                  <div className="git-review__repository-heading">{file.sourceAlias}</div>
                )}
              <button
                className="git-review__file-list-item"
                type="button"
                data-selected={selectedFileId === file.id ? 'true' : undefined}
                key={file.id}
                title={file.path}
                onClick={() => onSelect(file.id)}
              >
                <GitReviewFileIcon path={file.path} />
                <span className="git-review__file-list-name">
                  <span>{name}</span>
                  {directory && <small>{directory}</small>}
                </span>
              </button>
            </Fragment>
          )
        })}
      </div>
    </aside>
  )
}

type ReviewHook = ReturnType<typeof useGitReview>

interface GitReviewContentProps {
  capabilities: GitReviewTargetCapabilities
  diffLayoutWidth: number
  diffStates: ReviewHook['diffStates']
  expandedFileIds: Set<string>
  fileElementsRef: React.MutableRefObject<Map<string, HTMLElement>>
  fileContentStates: ReviewHook['fileContentStates']
  filteredFiles: GitReviewFile[]
  hasSearchQuery: boolean
  isActive: boolean
  loadFullFiles: boolean
  mutateFile: (fileId: string, action: Parameters<ReviewHook['mutateFile']>[1]) => void
  nearFileIds: Set<string>
  onCopyFile: (path: string, file: GitReviewFile) => Promise<void>
  onOpenFile: (path: string, file: GitReviewFile) => void
  onRestore: (file: GitReviewFile) => void
  onRefresh: ReviewHook['refresh']
  pendingFileId: string | null
  targetKind: GitReviewTarget['kind']
  scrollRootRef: React.RefObject<HTMLDivElement | null>
  selectedFileId: string | null
  retryFileDiff: ReviewHook['retryFileDiff']
  reviewSnapshotId?: string
  summaryState: ReviewHook['summaryState']
  t: ReturnType<typeof useFrontendConfig>['t']
  toggleFile: (fileId: string) => void
  viewMode: GitReviewViewMode
  visibleFileIds: Set<string>
  wrapLines: boolean
}

function GitReviewContent({
  capabilities,
  diffLayoutWidth,
  diffStates,
  expandedFileIds,
  fileElementsRef,
  fileContentStates,
  filteredFiles,
  hasSearchQuery,
  isActive,
  loadFullFiles,
  mutateFile,
  nearFileIds,
  onCopyFile,
  onOpenFile,
  onRestore,
  onRefresh,
  pendingFileId,
  targetKind,
  scrollRootRef,
  selectedFileId,
  retryFileDiff,
  reviewSnapshotId,
  summaryState,
  t,
  toggleFile,
  viewMode,
  visibleFileIds,
  wrapLines
}: GitReviewContentProps): ReactNode {
  if (
    (summaryState.status === 'idle' || summaryState.status === 'loading') &&
    !summaryState.value
  ) {
    return (
      <div className="git-review__center-state" role="status">
        <LoaderCircle className="git-review__spinner" aria-hidden="true" />
        <span>{t('gitReview.loading')}</span>
      </div>
    )
  }

  if (summaryState.status === 'error' && !summaryState.value) {
    return (
      <div className="git-review__center-state git-review__center-state--error" role="alert">
        <AlertCircle aria-hidden="true" />
        <h2>{t('gitReview.error.title')}</h2>
        <p>{summaryState.error}</p>
        <button type="button" onClick={() => void onRefresh()}>
          <RefreshCw aria-hidden="true" />
          {t('gitReview.retry')}
        </button>
      </div>
    )
  }

  if (summaryState.value && summaryState.value.files.length === 0) {
    const { description, title } = gitReviewEmptyStateCopy(targetKind, t)
    return (
      <div className="git-review__center-state git-review__center-state--empty">
        <h2>{title}</h2>
        <p>{description}</p>
      </div>
    )
  }

  if (hasSearchQuery && filteredFiles.length === 0) {
    return (
      <div className="git-review__center-state git-review__center-state--empty">
        <h2>{t('gitReview.search.noMatches')}</h2>
        <p>{t('gitReview.search.noMatchesDescription')}</p>
      </div>
    )
  }

  return (
    <div className="git-review__diff-list">
      {filteredFiles.map((file, index) => (
        <div
          data-review-file-id={file.id}
          key={file.id}
          ref={(element) => {
            if (element) fileElementsRef.current.set(file.id, element)
            else fileElementsRef.current.delete(file.id)
          }}
        >
          {summaryState.value?.source?.kind === 'all' &&
            (index === 0 || filteredFiles[index - 1].sourceFolderId !== file.sourceFolderId) && (
              <div className="git-review__repository-heading">{file.sourceAlias}</div>
            )}
          <GitReviewDiffCard
            capabilities={capabilities}
            diffState={diffStates[file.id]}
            file={file}
            fileContentState={fileContentStates[file.id]}
            isExpanded={expandedFileIds.has(file.id)}
            isNearViewport={nearFileIds.has(file.id)}
            isReviewActive={isActive}
            isSelected={selectedFileId === file.id}
            isVisible={visibleFileIds.has(file.id)}
            layoutWidth={diffLayoutWidth}
            loadFullFiles={loadFullFiles}
            mutationLocked={pendingFileId !== null}
            mutationPending={pendingFileId === file.id}
            onCopyFile={(path) => onCopyFile(path, file)}
            onMutate={mutateFile}
            onOpenFile={(path) => onOpenFile(path, file)}
            onRequestDiff={retryFileDiff}
            onRestore={onRestore}
            onToggle={toggleFile}
            scrollRootRef={scrollRootRef}
            reviewSnapshotId={reviewSnapshotId}
            t={t}
            viewMode={viewMode}
            wrapLines={wrapLines}
          />
        </div>
      ))}
    </div>
  )
}

function gitReviewEmptyStateCopy(
  kind: GitReviewTarget['kind'],
  t: ReturnType<typeof useFrontendConfig>['t']
): { description: string; title: string } {
  switch (kind) {
    case 'lastTurn':
      return {
        description: t('gitReview.empty.lastTurn.description'),
        title: t('gitReview.empty.lastTurn.title')
      }
    case 'uncommitted':
      return {
        description: t('gitReview.empty.uncommitted.description'),
        title: t('gitReview.empty.uncommitted.title')
      }
    case 'unstaged':
      return {
        description: t('gitReview.empty.unstaged.description'),
        title: t('gitReview.empty.unstaged.title')
      }
    case 'staged':
      return {
        description: t('gitReview.empty.staged.description'),
        title: t('gitReview.empty.staged.title')
      }
    case 'commit':
      return {
        description: t('gitReview.empty.commit.description'),
        title: t('gitReview.empty.commit.title')
      }
    case 'branch':
      return {
        description: t('gitReview.empty.branch.description'),
        title: t('gitReview.empty.branch.title')
      }
  }
}

function setsEqual(left: Set<string>, right: Set<string>): boolean {
  if (left.size !== right.size) return false
  return [...left].every((value) => right.has(value))
}

function updateMembership(values: Set<string>, value: string, included: boolean): void {
  if (included) values.add(value)
  else values.delete(value)
}

function isManualScrollKey(key: string): boolean {
  return (
    key === ' ' ||
    key === 'ArrowDown' ||
    key === 'ArrowUp' ||
    key === 'End' ||
    key === 'Home' ||
    key === 'PageDown' ||
    key === 'PageUp'
  )
}
