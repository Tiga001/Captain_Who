import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type RefObject
} from 'react'
import type {
  GitReviewFile,
  GitReviewFileMutationAction,
  GitReviewScope
} from '@mycopilot/protocol'
import {
  ChevronDown,
  ChevronRight,
  ExternalLink,
  LoaderCircle,
  Minus,
  Plus,
  Undo2
} from 'lucide-react'
import { Tooltip } from '../../components/overlay/Tooltip'
import type { Translate } from '../../config/translationFormat'
import { GitReviewDiffRenderer } from './GitReviewDiffRenderer'
import type { GitReviewDiffExpandHandler } from './GitReviewDiffRenderer'
import { reduceGitDiffExpansion, type GitDiffExpansionState } from './diff'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import { canHydrateGitReviewFile } from './gitReviewFileCapabilities'
import type { GitReviewViewMode } from './gitReviewViewMode'
import type { GitReviewDiffState, GitReviewFileContentState } from './useGitReview'

const EMPTY_EXPANSION_STATE: GitDiffExpansionState = new Map()
const DEFAULT_DIFF_BODY_HEIGHT = 87
const DIFF_BODY_UNMOUNT_DELAY_MS = 400

interface GitReviewDiffCardProps {
  diffState?: GitReviewDiffState
  file: GitReviewFile
  fileContentState?: GitReviewFileContentState
  isExpanded: boolean
  isNearViewport: boolean
  isReviewActive: boolean
  isSelected: boolean
  isVisible: boolean
  layoutWidth?: number
  loadFullFiles: boolean
  mutationLocked: boolean
  mutationPending: boolean
  onMutate: (fileId: string, action: GitReviewFileMutationAction) => void
  onOpenFile: (path: string) => void
  onRequestDiff: (fileId: string) => void
  onRestore: (file: GitReviewFile) => void
  onToggle: (fileId: string) => void
  scope: GitReviewScope
  scrollRootRef: RefObject<HTMLDivElement | null>
  reviewSnapshotId?: string
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

/** Owns file-level review UX; patch parsing and line rendering live behind a separate boundary. */
export const GitReviewDiffCard = memo(function GitReviewDiffCard({
  diffState,
  file,
  fileContentState,
  isExpanded,
  isNearViewport,
  isReviewActive,
  isSelected,
  isVisible,
  layoutWidth = 0,
  loadFullFiles,
  mutationLocked,
  mutationPending,
  onMutate,
  onOpenFile,
  onRequestDiff,
  onRestore,
  onToggle,
  scope,
  scrollRootRef,
  reviewSnapshotId,
  t,
  viewMode,
  wrapLines
}: GitReviewDiffCardProps): ReactNode {
  const cardRef = useRef<HTMLElement>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const pendingScrollAnchorRef = useRef<{ anchorId: string; beforeTop: number } | null>(null)
  const [retainNearBody, setRetainNearBody] = useState(isNearViewport)
  const activeDiff = diffState?.status === 'ready' ? diffState.value : null
  const [expansionController, setExpansionController] = useState<{
    diff: typeof activeDiff
    state: GitDiffExpansionState
  }>(() => ({ diff: activeDiff, state: EMPTY_EXPANSION_STATE }))
  const expansionState =
    expansionController.diff === activeDiff ? expansionController.state : EMPTY_EXPANSION_STATE

  const syntaxSourceReady =
    !loadFullFiles ||
    !canHydrateGitReviewFile(file.status) ||
    fileContentState?.status === 'ready' ||
    fileContentState?.status === 'error'

  useEffect(() => {
    if (!isReviewActive || !isExpanded) {
      setRetainNearBody(false)
      return undefined
    }
    if (isNearViewport || isSelected) {
      setRetainNearBody(true)
      return undefined
    }
    const timeout = window.setTimeout(() => setRetainNearBody(false), DIFF_BODY_UNMOUNT_DELAY_MS)
    return () => window.clearTimeout(timeout)
  }, [isExpanded, isNearViewport, isReviewActive, isSelected])

  const expansionSignature = useMemo(
    () =>
      [...expansionState]
        .map(([gapId, value]) => `${gapId}:${value.fromStart}:${value.fromEnd}`)
        .join('|'),
    [expansionState]
  )
  const bodySnapshotKey = `${file.id}:${reviewSnapshotId ?? activeDiff?.snapshotId ?? 'pending'}`
  const bodyLayoutKey = `${bodySnapshotKey}:${viewMode}:${wrapLines ? 1 : 0}:${Math.round(layoutWidth)}:${expansionSignature}`
  const [bodyMeasurement, setBodyMeasurement] = useState({
    height: DEFAULT_DIFF_BODY_HEIGHT,
    key: bodyLayoutKey,
    snapshotKey: bodySnapshotKey
  })
  const reservedBodyHeight =
    bodyMeasurement.key === bodyLayoutKey || bodyMeasurement.snapshotKey === bodySnapshotKey
      ? bodyMeasurement.height
      : DEFAULT_DIFF_BODY_HEIGHT
  const shouldRenderBody =
    isExpanded && isReviewActive && (isNearViewport || isSelected || retainNearBody)
  const shouldPreserveLoadingHeight =
    !diffState || diffState.status === 'idle' || diffState.status === 'loading'

  useLayoutEffect(() => {
    if (!shouldRenderBody) return undefined
    const body = bodyRef.current
    if (!body) return undefined
    const measure = (): void => {
      const height = Math.max(
        DEFAULT_DIFF_BODY_HEIGHT,
        Math.ceil(body.getBoundingClientRect().height)
      )
      setBodyMeasurement((current) =>
        current.key === bodyLayoutKey && Math.abs(current.height - height) <= 1
          ? current
          : { height, key: bodyLayoutKey, snapshotKey: bodySnapshotKey }
      )
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(measure)
    observer.observe(body)
    return () => observer.disconnect()
  }, [bodyLayoutKey, bodySnapshotKey, shouldRenderBody])

  const handleExpand = useCallback<GitReviewDiffExpandHandler>(
    (action, scrollAnchorId) => {
      if (!activeDiff || action.type !== 'expand') return
      if (scrollAnchorId) {
        const anchor = findPrimaryDiffAnchor(cardRef.current, scrollAnchorId)
        if (anchor) {
          pendingScrollAnchorRef.current = {
            anchorId: scrollAnchorId,
            beforeTop: anchor.getBoundingClientRect().top
          }
        }
      }
      setExpansionController((current) => ({
        diff: activeDiff,
        state: reduceGitDiffExpansion(
          current.diff === activeDiff ? current.state : EMPTY_EXPANSION_STATE,
          action
        )
      }))
    },
    [activeDiff]
  )

  useLayoutEffect(() => {
    const pending = pendingScrollAnchorRef.current
    if (!pending) return
    pendingScrollAnchorRef.current = null
    const scrollRoot = scrollRootRef.current
    const anchor = findPrimaryDiffAnchor(cardRef.current, pending.anchorId)
    if (!scrollRoot || !anchor) return
    const offset = anchor.getBoundingClientRect().top - pending.beforeTop
    if (Math.abs(offset) > 0.5) scrollRoot.scrollTop += offset
  }, [expansionState, scrollRootRef])

  const pathTitle = file.previousPath ? `${file.previousPath} → ${file.path}` : file.path
  const actionLabel = isExpanded ? t('gitReview.file.collapse') : t('gitReview.file.expand')
  const statusLabel = t(`gitReview.status.${file.status}`)
  const statsLabel = file.stats ? `, +${file.stats.additions} -${file.stats.deletions}` : ''

  return (
    <section
      className="git-review__diff-card"
      data-expanded={isExpanded ? 'true' : undefined}
      data-file-id={file.id}
      ref={cardRef}
    >
      <div className="git-review__diff-card-header">
        <button
          className="git-review__diff-card-main"
          type="button"
          aria-expanded={isExpanded}
          aria-label={`${actionLabel}: ${pathTitle}, ${statusLabel}${statsLabel}`}
          title={pathTitle}
          onClick={() => onToggle(file.id)}
        >
          <GitReviewFileIcon path={file.path} />
          <GitReviewFilePath file={file} />
          {file.stats && <GitReviewFileStats file={file} />}
        </button>
        <div className="git-review__file-actions" aria-label={t('gitReview.file.actions')}>
          <FileActionButton label={actionLabel} onClick={() => onToggle(file.id)}>
            {isExpanded ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}
          </FileActionButton>
          <FileActionButton label={t('gitReview.file.open')} onClick={() => onOpenFile(file.path)}>
            <ExternalLink aria-hidden="true" />
          </FileActionButton>
          {scope === 'unstaged' && (
            <FileActionButton
              disabled={mutationLocked}
              label={t('gitReview.file.restore')}
              onClick={() => onRestore(file)}
            >
              <Undo2 aria-hidden="true" />
            </FileActionButton>
          )}
          {scope !== 'lastTurn' && (
            <FileActionButton
              disabled={mutationLocked}
              label={scope === 'unstaged' ? t('gitReview.file.stage') : t('gitReview.file.unstage')}
              onClick={() => onMutate(file.id, scope === 'unstaged' ? 'stage' : 'unstage')}
            >
              {mutationPending ? (
                <LoaderCircle className="git-review__spinner" aria-hidden="true" />
              ) : scope === 'unstaged' ? (
                <Plus aria-hidden="true" />
              ) : (
                <Minus aria-hidden="true" />
              )}
            </FileActionButton>
          )}
        </div>
      </div>

      {isExpanded && shouldRenderBody && (
        <div
          className="git-review__diff-card-body"
          ref={bodyRef}
          style={shouldPreserveLoadingHeight ? { minHeight: reservedBodyHeight } : undefined}
        >
          <GitReviewDiffRenderer
            diffState={diffState}
            expansionState={expansionState}
            file={file}
            fileContentState={loadFullFiles ? fileContentState : undefined}
            onRequestDiff={onRequestDiff}
            onExpand={handleExpand}
            syntaxHighlightingEnabled={isReviewActive && isVisible && syntaxSourceReady}
            t={t}
            viewMode={viewMode}
            wrapLines={wrapLines}
          />
        </div>
      )}
      {isExpanded && !shouldRenderBody && (
        <div
          className="git-review__diff-card-body git-review__diff-card-body--placeholder"
          aria-hidden="true"
          style={{ height: reservedBodyHeight }}
        />
      )}
    </section>
  )
})

function findPrimaryDiffAnchor(root: HTMLElement | null, anchorId: string): HTMLElement | null {
  if (!root) return null
  return (
    Array.from(root.querySelectorAll<HTMLElement>('[data-primary-anchor="true"]')).find(
      (candidate) => candidate.dataset.diffAnchorId === anchorId
    ) ?? null
  )
}

function GitReviewFilePath({ file }: { file: GitReviewFile }): ReactNode {
  const separatorIndex = file.path.lastIndexOf('/')
  const directory = separatorIndex >= 0 ? file.path.slice(0, separatorIndex + 1) : ''
  const name = separatorIndex >= 0 ? file.path.slice(separatorIndex + 1) : file.path

  return (
    <span className="git-review__file-path">
      <bdi>
        <span className="git-review__file-directory">
          {file.previousPath && `${file.previousPath} → `}
          {directory}
        </span>
        <span className="git-review__file-name">{name}</span>
      </bdi>
    </span>
  )
}

interface FileActionButtonProps {
  ariaDisabled?: boolean
  children: ReactNode
  disabled?: boolean
  label: string
  onClick: () => void
}

function FileActionButton({
  ariaDisabled,
  children,
  disabled,
  label,
  onClick
}: FileActionButtonProps): ReactNode {
  return (
    <Tooltip anchorClassName="git-review__file-action-wrap" content={label}>
      <button
        className="git-review__file-action-button"
        type="button"
        aria-disabled={ariaDisabled || undefined}
        aria-label={label}
        disabled={disabled}
        onClick={(event) => {
          event.stopPropagation()
          if (!ariaDisabled) onClick()
        }}
      >
        {children}
      </button>
    </Tooltip>
  )
}

function GitReviewFileStats({ file }: { file: GitReviewFile }): ReactNode {
  if (!file.stats) return null
  return (
    <span className="git-review__file-stats">
      <span className="git-review__file-additions">+{file.stats.additions}</span>
      <span className="git-review__file-deletions">-{file.stats.deletions}</span>
    </span>
  )
}
