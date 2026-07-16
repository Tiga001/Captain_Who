import {
  useCallback,
  useEffect,
  useLayoutEffect,
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
import type { GitReviewViewMode } from './gitReviewViewMode'
import type { GitReviewDiffState, GitReviewFileContentState } from './useGitReview'

const EMPTY_EXPANSION_STATE: GitDiffExpansionState = new Map()

interface GitReviewDiffCardProps {
  diffState?: GitReviewDiffState
  file: GitReviewFile
  fileContentState?: GitReviewFileContentState
  isExpanded: boolean
  isReviewActive: boolean
  isVisible: boolean
  loadFullFiles: boolean
  mutationLocked: boolean
  mutationPending: boolean
  onMutate: (fileId: string, action: GitReviewFileMutationAction) => void
  onRequestDiff: (fileId: string) => void
  onRequestFileContent: (fileId: string) => void
  onRestore: (file: GitReviewFile) => void
  onToggle: (fileId: string) => void
  scope: GitReviewScope
  scrollRootRef: RefObject<HTMLDivElement | null>
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

/** Owns file-level review UX; patch parsing and line rendering live behind a separate boundary. */
export function GitReviewDiffCard({
  diffState,
  file,
  fileContentState,
  isExpanded,
  isReviewActive,
  isVisible,
  loadFullFiles,
  mutationLocked,
  mutationPending,
  onMutate,
  onRequestDiff,
  onRequestFileContent,
  onRestore,
  onToggle,
  scope,
  scrollRootRef,
  t,
  viewMode,
  wrapLines
}: GitReviewDiffCardProps): ReactNode {
  const cardRef = useRef<HTMLElement>(null)
  const pendingScrollAnchorRef = useRef<{ anchorId: string; beforeTop: number } | null>(null)
  const activeDiff = diffState?.status === 'ready' ? diffState.value : null
  const [expansionController, setExpansionController] = useState<{
    diff: typeof activeDiff
    state: GitDiffExpansionState
  }>(() => ({ diff: activeDiff, state: EMPTY_EXPANSION_STATE }))
  const expansionState =
    expansionController.diff === activeDiff ? expansionController.state : EMPTY_EXPANSION_STATE

  useEffect(() => {
    if (isExpanded) onRequestDiff(file.id)
  }, [file.id, isExpanded, onRequestDiff])

  const canLoadFullContent =
    isReviewActive &&
    isExpanded &&
    isVisible &&
    loadFullFiles &&
    isFullContentEligible(file.status) &&
    diffState?.status === 'ready' &&
    diffState.value.status === 'ready' &&
    (!fileContentState || fileContentState.status === 'idle')

  useEffect(() => {
    if (canLoadFullContent && isElementVisibleWithinRoot(cardRef.current, scrollRootRef.current)) {
      onRequestFileContent(file.id)
    }
  }, [canLoadFullContent, file.id, onRequestFileContent, scrollRootRef])

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
          <FileActionButton
            ariaDisabled
            label={t('gitReview.file.openSoon')}
            onClick={() => undefined}
          >
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
        </div>
      </div>

      {isExpanded && (
        <div className="git-review__diff-card-body">
          <GitReviewDiffRenderer
            diffState={diffState}
            expansionState={expansionState}
            fileContentState={loadFullFiles ? fileContentState : undefined}
            fileId={file.id}
            fileStatus={file.status}
            onRequestDiff={onRequestDiff}
            onExpand={handleExpand}
            t={t}
            viewMode={viewMode}
            wrapLines={wrapLines}
          />
        </div>
      )}
    </section>
  )
}

function isFullContentEligible(status: GitReviewFile['status']): boolean {
  return status === 'modified' || status === 'renamed' || status === 'copied'
}

function findPrimaryDiffAnchor(root: HTMLElement | null, anchorId: string): HTMLElement | null {
  if (!root) return null
  return (
    Array.from(root.querySelectorAll<HTMLElement>('[data-primary-anchor="true"]')).find(
      (candidate) => candidate.dataset.diffAnchorId === anchorId
    ) ?? null
  )
}

function isElementVisibleWithinRoot(
  element: HTMLElement | null,
  root: HTMLElement | null
): boolean {
  if (!element || !root) return false
  const elementRect = element.getBoundingClientRect()
  const rootRect = root.getBoundingClientRect()
  return (
    elementRect.width > 0 &&
    elementRect.height > 0 &&
    elementRect.right > rootRect.left &&
    elementRect.left < rootRect.right &&
    elementRect.bottom > rootRect.top &&
    elementRect.top < rootRect.bottom
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
