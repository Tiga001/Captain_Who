import type { ReactNode } from 'react'
import type { GitReviewFile, GitReviewTarget } from '@mycopilot/protocol'
import { AlertCircle, LoaderCircle, RefreshCw } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { GitReviewDiffCard } from './GitReviewDiffCard'
import { type GitReviewTargetCapabilities } from './gitReviewTargetCapabilities'
import type { GitReviewViewMode } from './gitReviewViewMode'
import { useGitReview } from './useGitReview'

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

export function GitReviewContent({
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
