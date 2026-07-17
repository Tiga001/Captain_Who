import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type { GitReviewFile, GitReviewScope } from '@mycopilot/protocol'
import {
  AlertCircle,
  Check,
  ChevronDown,
  Ellipsis,
  FileSearch,
  FileText,
  FolderOpen,
  ListCollapse,
  LoaderCircle,
  RefreshCw,
  Search,
  WrapText,
  X
} from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { dismissActiveTooltip, Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewDiffCard } from './GitReviewDiffCard'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import { canHydrateGitReviewFile } from './gitReviewFileCapabilities'
import { loadGitReviewPreferences, saveGitReviewPreferences } from './gitReviewPreferences'
import { getTargetGitReviewViewMode } from './gitReviewViewMode'
import type { GitReviewViewMode } from './gitReviewViewMode'
import { useGitReview } from './useGitReview'
import './GitReviewPanel.css'

interface GitReviewPanelProps {
  isActive: boolean
  projectId: string
}

type OpenMenu = 'scope' | 'options' | null

interface PendingFileAlignment {
  fileId: string
  token: number
}

interface GitReviewFileVisibility {
  near: Set<string>
  visible: Set<string>
}

function emptyFileVisibility(): GitReviewFileVisibility {
  return { near: new Set(), visible: new Set() }
}

export function GitReviewPanel({ isActive, projectId }: GitReviewPanelProps): ReactNode {
  const { t } = useFrontendConfig()
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
    scope,
    setHotDiffFileIds,
    setHotFullContentFileIds,
    setScope,
    summaryState
  } = useGitReview(projectId, isActive)
  const [expandedFileIds, setExpandedFileIds] = useState<Set<string>>(() => new Set())
  const [selectedFileId, setSelectedFileId] = useState<string | null>(null)
  const [viewMode, setViewMode] = useState<GitReviewViewMode>('unified')
  const [wrapLines, setWrapLines] = useState(false)
  const [reviewPreferences, setReviewPreferences] = useState(loadGitReviewPreferences)
  const [showFileList, setShowFileList] = useState(false)
  const [showSearch, setShowSearch] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null)
  const [restoreCandidate, setRestoreCandidate] = useState<GitReviewFile | null>(null)
  const [fileVisibility, setFileVisibility] = useState<GitReviewFileVisibility>(emptyFileVisibility)
  const [diffLayoutWidth, setDiffLayoutWidth] = useState(0)
  const contentRef = useRef<HTMLDivElement>(null)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const fileElementsRef = useRef(new Map<string, HTMLElement>())
  const diffStatesRef = useRef(diffStates)
  const initializedQueryRef = useRef<string | null>(null)
  const pendingFileAlignmentRef = useRef<PendingFileAlignment | null>(null)
  const fileAlignmentSequenceRef = useRef(0)
  const files = useMemo(() => summaryState.value?.files ?? [], [summaryState.value])

  const cancelPendingFileAlignment = useCallback(() => {
    pendingFileAlignmentRef.current = null
    contentRef.current?.scrollBy({ behavior: 'auto', left: 0, top: 0 })
  }, [])

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
    cancelPendingFileAlignment()
  }, [cancelPendingFileAlignment, projectId, scope])

  useEffect(() => {
    if (!openMenu) return
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target
      if (!(target instanceof Element)) return
      if (
        target.closest('.git-review__menu') ||
        target.closest('.git-review__scope-control') ||
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
    if (!summaryState.value || summaryState.value.scope !== scope) return
    const fileIds = new Set(files.map((file) => file.id))
    const queryKey = `${projectId}:${scope}`
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
  }, [files, projectId, scope, summaryState.value])

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

  const handleMutateFile = useCallback(
    (fileId: string, action: Parameters<typeof mutateFile>[1]) => {
      void mutateFile(fileId, action).catch(() => undefined)
    },
    [mutateFile]
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

  const handleScopeChange = useCallback(
    (nextScope: GitReviewScope) => {
      if (nextScope !== scope) setScope(nextScope)
      setOpenMenu(null)
    },
    [scope, setScope]
  )

  const toggleSearch = useCallback(() => {
    setShowSearch((current) => {
      if (current) setSearchQuery('')
      return !current
    })
    setOpenMenu(null)
  }, [])

  const scopeLabel =
    scope === 'unstaged' ? t('gitReview.scope.unstaged') : t('gitReview.scope.staged')
  const stats = summaryState.value?.scope === scope ? summaryState.value.stats : null
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
      <header className="git-review__toolbar">
        <div className="git-review__toolbar-summary">
          <div className="git-review__scope-control">
            <button
              className="git-review__scope-button"
              type="button"
              aria-expanded={openMenu === 'scope'}
              aria-haspopup="menu"
              onClick={() => setOpenMenu((current) => (current === 'scope' ? null : 'scope'))}
            >
              <span className="git-review__scope-label">{scopeLabel}</span>
              {stats && <span className="git-review__file-count">{stats.fileCount}</span>}
              <ChevronDown aria-hidden="true" />
            </button>
            {openMenu === 'scope' && (
              <div className="git-review__menu git-review__scope-menu" role="menu">
                <ScopeMenuItem
                  checked={scope === 'unstaged'}
                  label={t('gitReview.scope.unstaged')}
                  onSelect={() => handleScopeChange('unstaged')}
                />
                <ScopeMenuItem
                  checked={scope === 'staged'}
                  label={t('gitReview.scope.staged')}
                  onSelect={() => handleScopeChange('staged')}
                />
              </div>
            )}
          </div>
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
              label={t('gitReview.options')}
              onClick={() => setOpenMenu((current) => (current === 'options' ? null : 'options'))}
            >
              <Ellipsis aria-hidden="true" />
            </ToolbarButton>
            {openMenu === 'options' && (
              <div className="git-review__menu git-review__options-menu" role="menu">
                <button
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setOpenMenu(null)
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
                    setOpenMenu(null)
                  }}
                >
                  <WrapText aria-hidden="true" />
                  {t('gitReview.wrapLines')}
                  {wrapLines && <Check className="git-review__menu-check" aria-hidden="true" />}
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
                    setOpenMenu(null)
                  }}
                >
                  <FileText aria-hidden="true" />
                  {reviewPreferences.loadFullFiles
                    ? t('gitReview.dontLoadFullFiles')
                    : t('gitReview.loadFullFiles')}
                </button>
              </div>
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
            onRestore={setRestoreCandidate}
            nearFileIds={fileVisibility.near}
            scope={scope}
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
  children: ReactNode
  disabled?: boolean
  label: string
  onClick: () => void
}

function ToolbarButton({ active, children, disabled, label, onClick }: ToolbarButtonProps) {
  return (
    <Tooltip
      anchorClassName="git-review__toolbar-button-wrap"
      content={label}
      preferredPlacement="bottom"
    >
      <button
        className="git-review__toolbar-button"
        type="button"
        aria-label={label}
        aria-pressed={active === undefined ? undefined : active}
        data-active={active ? 'true' : undefined}
        disabled={disabled}
        onClick={onClick}
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

function ScopeMenuItem({
  checked,
  label,
  onSelect
}: {
  checked: boolean
  label: string
  onSelect: () => void
}): ReactNode {
  return (
    <button type="button" role="menuitemradio" aria-checked={checked} onClick={onSelect}>
      <span>{label}</span>
      {checked && <Check className="git-review__menu-check" aria-hidden="true" />}
    </button>
  )
}

interface FileListProps {
  files: GitReviewFile[]
  onSelect: (fileId: string) => void
  selectedFileId: string | null
  t: ReturnType<typeof useFrontendConfig>['t']
}

function FileList({ files, onSelect, selectedFileId, t }: FileListProps): ReactNode {
  return (
    <aside className="git-review__file-list" aria-label={t('gitReview.fileList')}>
      <div className="git-review__file-list-heading">
        <span>{t('gitReview.fileList')}</span>
        <span>{files.length}</span>
      </div>
      <div className="git-review__file-list-scroll">
        {files.map((file) => {
          const pathParts = file.path.split('/')
          const name = pathParts.pop() || file.path
          const directory = pathParts.join('/')
          return (
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
          )
        })}
      </div>
    </aside>
  )
}

type ReviewHook = ReturnType<typeof useGitReview>

interface GitReviewContentProps {
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
  onRestore: (file: GitReviewFile) => void
  onRefresh: ReviewHook['refresh']
  pendingFileId: string | null
  scope: GitReviewScope
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
  onRestore,
  onRefresh,
  pendingFileId,
  scope,
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
    const title =
      scope === 'unstaged' ? t('gitReview.empty.unstaged.title') : t('gitReview.empty.staged.title')
    const description =
      scope === 'unstaged'
        ? t('gitReview.empty.unstaged.description')
        : t('gitReview.empty.staged.description')
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
      {filteredFiles.map((file) => (
        <div
          data-review-file-id={file.id}
          key={file.id}
          ref={(element) => {
            if (element) fileElementsRef.current.set(file.id, element)
            else fileElementsRef.current.delete(file.id)
          }}
        >
          <GitReviewDiffCard
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
            onMutate={mutateFile}
            onRequestDiff={retryFileDiff}
            onRestore={onRestore}
            onToggle={toggleFile}
            scope={scope}
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
