import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import type { GitReviewCommit, GitReviewTarget } from '@mycopilot/protocol'
import { AlertCircle, Check, ChevronDown, ChevronRight, LoaderCircle } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { dismissActiveTooltip } from '../../components/overlay/Tooltip'
import { listGitReviewCommits } from './gitReviewClient'
import { GitReviewCommitSubmenu, type GitReviewCommitListState } from './GitReviewCommitSubmenu'
import { GitReviewMenuPortal } from './GitReviewMenuPortal'
import type { GitReviewRepositoryContextState } from './useGitReviewRepositoryContext'

interface GitReviewSourceSelectorProps {
  branchBaseRef?: string
  conversationId?: string | null
  fileCount?: number
  isOpen: boolean
  language: string
  onOpenChange: (open: boolean) => void
  onRequestRepositoryContext: (force?: boolean) => void
  onSelectBranch: () => void
  onSelectCommit: (commit: GitReviewCommit) => void
  onSelectTarget: (target: GitReviewTarget) => void
  projectId: string
  repositoryState: GitReviewRepositoryContextState
  t: Translate
  target: GitReviewTarget
}

export function GitReviewSourceSelector({
  branchBaseRef,
  conversationId,
  fileCount,
  isOpen,
  language,
  onOpenChange,
  onRequestRepositoryContext,
  onSelectBranch,
  onSelectCommit,
  onSelectTarget,
  projectId,
  repositoryState,
  t,
  target
}: GitReviewSourceSelectorProps): ReactNode {
  const triggerRef = useRef<HTMLButtonElement>(null)
  const commitItemRef = useRef<HTMLButtonElement>(null)
  const commitRequestRef = useRef(0)
  const commitStateRef = useRef<GitReviewCommitListState>({ status: 'idle' })
  const [isCommitMenuOpen, setIsCommitMenuOpen] = useState(false)
  const [commitMenuAutoFocus, setCommitMenuAutoFocus] = useState(false)
  const [commitState, setCommitState] = useState<GitReviewCommitListState>({ status: 'idle' })
  const [pendingBranchSelection, setPendingBranchSelection] = useState(false)
  const label = sourceLabel(target.kind, t)

  useEffect(() => {
    commitRequestRef.current += 1
    const idle = { status: 'idle' } as const
    commitStateRef.current = idle
    setCommitState(idle)
    setIsCommitMenuOpen(false)
    setPendingBranchSelection(false)
  }, [projectId])

  useEffect(
    () => () => {
      commitRequestRef.current += 1
    },
    []
  )

  useEffect(() => {
    if (!isOpen) {
      setIsCommitMenuOpen(false)
      setPendingBranchSelection(false)
    }
  }, [isOpen])

  const loadCommits = useCallback(
    async (force = false): Promise<void> => {
      if (!projectId) return
      if (
        !force &&
        (commitStateRef.current.status === 'loading' || commitStateRef.current.status === 'ready')
      ) {
        return
      }
      const requestId = commitRequestRef.current + 1
      commitRequestRef.current = requestId
      const loading = { status: 'loading' } as const
      commitStateRef.current = loading
      setCommitState(loading)
      try {
        const result = await listGitReviewCommits({ projectId })
        if (commitRequestRef.current !== requestId) return
        const ready = {
          commits: result.commits,
          status: 'ready',
          truncated: result.truncated
        } as const
        commitStateRef.current = ready
        setCommitState(ready)
      } catch (error) {
        if (commitRequestRef.current !== requestId) return
        const failed = {
          error: error instanceof Error ? error.message : String(error),
          status: 'error'
        } as const
        commitStateRef.current = failed
        setCommitState(failed)
      }
    },
    [projectId]
  )

  const focusTrigger = useCallback(() => {
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }, [])

  const closeAll = useCallback(() => {
    setIsCommitMenuOpen(false)
    setPendingBranchSelection(false)
    onOpenChange(false)
    focusTrigger()
  }, [focusTrigger, onOpenChange])

  useEffect(() => {
    if (!pendingBranchSelection) return
    if (repositoryState.status === 'error') {
      setPendingBranchSelection(false)
      return
    }
    if (repositoryState.status !== 'ready') return
    setPendingBranchSelection(false)
    if (!branchBaseRef) return
    onSelectBranch()
    closeAll()
  }, [branchBaseRef, closeAll, onSelectBranch, pendingBranchSelection, repositoryState.status])

  const openCommitMenu = useCallback(
    (autoFocus: boolean) => {
      setCommitMenuAutoFocus(autoFocus)
      setIsCommitMenuOpen(true)
      void loadCommits()
    },
    [loadCommits]
  )

  const selectTarget = useCallback(
    (next: GitReviewTarget) => {
      onSelectTarget(next)
      closeAll()
    },
    [closeAll, onSelectTarget]
  )

  const handleOpenChange = (open: boolean): void => {
    dismissActiveTooltip()
    if (open) onRequestRepositoryContext()
    else {
      setIsCommitMenuOpen(false)
      setPendingBranchSelection(false)
    }
    onOpenChange(open)
  }

  const handleBranchSelection = (): void => {
    if (repositoryState.status === 'ready' && branchBaseRef) {
      onSelectBranch()
      closeAll()
      return
    }
    setPendingBranchSelection(true)
    if (repositoryState.status !== 'loading') onRequestRepositoryContext(true)
  }

  return (
    <div className="git-review__source-control">
      <button
        aria-expanded={isOpen}
        aria-haspopup="menu"
        aria-label={label}
        className="git-review__scope-button"
        ref={triggerRef}
        type="button"
        onClick={() => handleOpenChange(!isOpen)}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
          event.preventDefault()
          handleOpenChange(true)
        }}
      >
        <span className="git-review__scope-label">{label}</span>
        {fileCount !== undefined && (
          <span className="git-review__file-count" aria-hidden="true">
            {fileCount}
          </span>
        )}
        <ChevronDown aria-hidden="true" />
      </button>

      {isOpen && (
        <GitReviewMenuPortal
          anchorRef={triggerRef}
          ariaLabel={t('gitReview.source.menu')}
          autoFocus="selected"
          className="git-review__source-menu"
          onEscape={closeAll}
          placement="bottom-start"
        >
          <SourceMenuItem
            checked={target.kind === 'lastTurn'}
            disabled={!conversationId}
            label={t('gitReview.scope.lastTurn')}
            onPointerEnter={() => setIsCommitMenuOpen(false)}
            onSelect={() => conversationId && selectTarget({ kind: 'lastTurn', conversationId })}
          />
          <div className="git-review__menu-separator" role="separator" />
          <SourceMenuItem
            checked={target.kind === 'uncommitted'}
            label={t('gitReview.scope.uncommitted')}
            onPointerEnter={() => setIsCommitMenuOpen(false)}
            onSelect={() => selectTarget({ kind: 'uncommitted' })}
          />
          <SourceMenuItem
            checked={target.kind === 'unstaged'}
            label={t('gitReview.scope.unstaged')}
            onPointerEnter={() => setIsCommitMenuOpen(false)}
            onSelect={() => selectTarget({ kind: 'unstaged' })}
          />
          <SourceMenuItem
            checked={target.kind === 'staged'}
            label={t('gitReview.scope.staged')}
            onPointerEnter={() => setIsCommitMenuOpen(false)}
            onSelect={() => selectTarget({ kind: 'staged' })}
          />
          <div className="git-review__menu-separator" role="separator" />
          <button
            aria-checked={target.kind === 'commit'}
            aria-expanded={isCommitMenuOpen}
            aria-haspopup="menu"
            className="git-review__source-menu-item"
            ref={commitItemRef}
            role="menuitemradio"
            type="button"
            onClick={() => openCommitMenu(true)}
            onKeyDown={(event) => {
              if (event.key !== 'ArrowRight') return
              event.preventDefault()
              event.stopPropagation()
              openCommitMenu(true)
            }}
            onPointerEnter={() => openCommitMenu(false)}
          >
            <span>{t('gitReview.scope.committed')}</span>
            <span className="git-review__menu-trailing">
              {target.kind === 'commit' && (
                <Check className="git-review__menu-check" aria-hidden="true" />
              )}
              <ChevronRight aria-hidden="true" />
            </span>
          </button>
          <SourceMenuItem
            checked={target.kind === 'branch'}
            disabled={repositoryState.status === 'ready' && !branchBaseRef}
            label={t('gitReview.scope.branch')}
            onPointerEnter={() => setIsCommitMenuOpen(false)}
            onSelect={handleBranchSelection}
            trailing={
              repositoryState.status === 'loading' ? (
                <LoaderCircle className="git-review__spinner" aria-hidden="true" />
              ) : repositoryState.status === 'error' ? (
                <AlertCircle aria-hidden="true" />
              ) : undefined
            }
          />
        </GitReviewMenuPortal>
      )}

      {isOpen && isCommitMenuOpen && (
        <GitReviewCommitSubmenu
          anchorRef={commitItemRef}
          autoFocus={commitMenuAutoFocus}
          language={language}
          onCloseAll={closeAll}
          onCloseSubmenu={() => {
            setIsCommitMenuOpen(false)
            window.requestAnimationFrame(() => commitItemRef.current?.focus())
          }}
          onRetry={() => void loadCommits(true)}
          onSelect={(commit) => {
            onSelectCommit(commit)
            closeAll()
          }}
          selectedSha={target.kind === 'commit' ? target.commitSha : undefined}
          state={commitState}
          t={t}
        />
      )}
    </div>
  )
}

function SourceMenuItem({
  checked,
  disabled,
  label,
  onPointerEnter,
  onSelect,
  trailing
}: {
  checked: boolean
  disabled?: boolean
  label: string
  onPointerEnter: () => void
  onSelect: () => void
  trailing?: ReactNode
}): ReactNode {
  return (
    <button
      aria-checked={checked}
      className="git-review__source-menu-item"
      disabled={disabled}
      role="menuitemradio"
      type="button"
      onClick={onSelect}
      onFocus={onPointerEnter}
      onPointerEnter={onPointerEnter}
    >
      <span>{label}</span>
      <span className="git-review__menu-trailing">
        {trailing}
        {checked && <Check className="git-review__menu-check" aria-hidden="true" />}
      </span>
    </button>
  )
}

export function sourceLabel(kind: GitReviewTarget['kind'], t: Translate): string {
  switch (kind) {
    case 'lastTurn':
      return t('gitReview.scope.lastTurn')
    case 'uncommitted':
      return t('gitReview.scope.uncommitted')
    case 'unstaged':
      return t('gitReview.scope.unstaged')
    case 'staged':
      return t('gitReview.scope.staged')
    case 'commit':
      return t('gitReview.scope.committed')
    case 'branch':
      return t('gitReview.scope.branch')
  }
}
