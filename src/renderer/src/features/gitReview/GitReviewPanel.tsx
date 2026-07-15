import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type { GitReviewFile, GitReviewScope } from '@mycopilot/protocol'
import {
  AlertCircle,
  Check,
  ChevronDown,
  Ellipsis,
  FileSearch,
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
import { GitDiffCard } from './GitDiffView'
import type { GitReviewViewMode } from './GitDiffView'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import { useGitReview } from './useGitReview'
import './GitReviewPanel.css'

interface GitReviewPanelProps {
  isActive: boolean
  projectId: string
}

type OpenMenu = 'scope' | 'options' | null

export function GitReviewPanel({ isActive, projectId }: GitReviewPanelProps): ReactNode {
  const { t } = useFrontendConfig()
  const {
    diffStates,
    dismissMutationError,
    loadFileDiff,
    mutateFile,
    mutationError,
    pendingFileId,
    refresh,
    scope,
    setScope,
    summaryState
  } = useGitReview(projectId, isActive)
  const [expandedFileIds, setExpandedFileIds] = useState<Set<string>>(() => new Set())
  const [selectedFileId, setSelectedFileId] = useState<string | null>(null)
  const [viewMode, setViewMode] = useState<GitReviewViewMode>('unified')
  const [wrapLines, setWrapLines] = useState(false)
  const [showFileList, setShowFileList] = useState(false)
  const [showSearch, setShowSearch] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null)
  const [restoreCandidate, setRestoreCandidate] = useState<GitReviewFile | null>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const fileElementsRef = useRef(new Map<string, HTMLElement>())
  const initializedQueryRef = useRef<string | null>(null)
  const files = useMemo(() => summaryState.value?.files ?? [], [summaryState.value])

  useEffect(() => {
    if (!isActive) setOpenMenu(null)
  }, [isActive])

  useEffect(() => {
    setRestoreCandidate(null)
  }, [projectId, scope])

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

  const allExpanded = files.length > 0 && files.every((file) => expandedFileIds.has(file.id))

  const toggleFile = useCallback((fileId: string) => {
    setExpandedFileIds((current) => {
      const next = new Set(current)
      if (next.has(fileId)) next.delete(fileId)
      else next.add(fileId)
      return next
    })
    setSelectedFileId(fileId)
  }, [])

  const toggleAllFiles = useCallback(() => {
    setExpandedFileIds((current) =>
      files.length > 0 && files.every((file) => current.has(file.id))
        ? new Set()
        : new Set(files.map((file) => file.id))
    )
  }, [files])

  const selectFile = useCallback((fileId: string) => {
    setSelectedFileId(fileId)
    setExpandedFileIds((current) => {
      if (current.has(fileId)) return current
      const next = new Set(current)
      next.add(fileId)
      return next
    })
    requestAnimationFrame(() => {
      fileElementsRef.current.get(fileId)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    })
  }, [])

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
  const nextViewLabel =
    viewMode === 'unified' ? t('gitReview.view.switchSplit') : t('gitReview.view.switchUnified')

  return (
    <div className="git-review" ref={panelRef}>
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
            emphasized
            label={nextViewLabel}
            onClick={() => setViewMode((current) => (current === 'unified' ? 'split' : 'unified'))}
          >
            <DiffLayoutIcon mode={viewMode} />
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

        <div className="git-review__content">
          <GitReviewContent
            diffStates={diffStates}
            expandedFileIds={expandedFileIds}
            fileElementsRef={fileElementsRef}
            filteredFiles={filteredFiles}
            hasSearchQuery={Boolean(normalizedQuery)}
            loadFileDiff={loadFileDiff}
            mutateFile={mutateFile}
            pendingFileId={pendingFileId}
            onRestore={setRestoreCandidate}
            scope={scope}
            summaryState={summaryState}
            t={t}
            toggleFile={toggleFile}
            viewMode={viewMode}
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
  emphasized?: boolean
  label: string
  onClick: () => void
}

function ToolbarButton({
  active,
  children,
  disabled,
  emphasized,
  label,
  onClick
}: ToolbarButtonProps) {
  const tooltipId = useId()
  return (
    <span className="git-review__toolbar-button-wrap">
      <button
        className="git-review__toolbar-button"
        type="button"
        aria-describedby={tooltipId}
        aria-label={label}
        aria-pressed={active === undefined ? undefined : active}
        data-active={active || emphasized ? 'true' : undefined}
        disabled={disabled}
        onClick={onClick}
      >
        {children}
      </button>
      <span className="git-review__tooltip" id={tooltipId} role="tooltip">
        {label}
      </span>
    </span>
  )
}

function DiffLayoutIcon({ mode }: { mode: GitReviewViewMode }): ReactNode {
  return (
    <span className="git-review__layout-icon" data-mode={mode} aria-hidden="true">
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
  diffStates: ReviewHook['diffStates']
  expandedFileIds: Set<string>
  fileElementsRef: React.MutableRefObject<Map<string, HTMLElement>>
  filteredFiles: GitReviewFile[]
  hasSearchQuery: boolean
  loadFileDiff: ReviewHook['loadFileDiff']
  mutateFile: ReviewHook['mutateFile']
  onRestore: (file: GitReviewFile) => void
  onRefresh: ReviewHook['refresh']
  pendingFileId: string | null
  scope: GitReviewScope
  summaryState: ReviewHook['summaryState']
  t: ReturnType<typeof useFrontendConfig>['t']
  toggleFile: (fileId: string) => void
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

function GitReviewContent({
  diffStates,
  expandedFileIds,
  fileElementsRef,
  filteredFiles,
  hasSearchQuery,
  loadFileDiff,
  mutateFile,
  onRestore,
  onRefresh,
  pendingFileId,
  scope,
  summaryState,
  t,
  toggleFile,
  viewMode,
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
          key={file.id}
          ref={(element) => {
            if (element) fileElementsRef.current.set(file.id, element)
            else fileElementsRef.current.delete(file.id)
          }}
        >
          <GitDiffCard
            diffState={diffStates[file.id]}
            file={file}
            isExpanded={expandedFileIds.has(file.id)}
            mutationLocked={pendingFileId !== null}
            mutationPending={pendingFileId === file.id}
            onMutate={(fileId, action) => {
              void mutateFile(fileId, action).catch(() => undefined)
            }}
            onRequestDiff={loadFileDiff}
            onRestore={onRestore}
            onToggle={toggleFile}
            scope={scope}
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
