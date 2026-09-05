import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode
} from 'react'
import type { GitReviewBranch } from '@mycopilot/protocol'
import { Check, ChevronDown, LoaderCircle, RefreshCw, Search } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { dismissActiveTooltip, Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewMenuPortal } from './GitReviewMenuPortal'
import type { GitReviewRepositoryContextState } from './useGitReviewRepositoryContext'

interface GitReviewBranchPickerProps {
  isActive: boolean
  onRetry: () => void
  onSelect: (branch: GitReviewBranch) => void
  selectedRef: string
  state: GitReviewRepositoryContextState
  t: Translate
}

export function GitReviewBranchPicker({
  isActive,
  onRetry,
  onSelect,
  selectedRef,
  state,
  t
}: GitReviewBranchPickerProps): ReactNode {
  const triggerRef = useRef<HTMLButtonElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const [isOpen, setIsOpen] = useState(false)
  const [query, setQuery] = useState('')
  const selectedName =
    state.status === 'ready'
      ? (state.value.branches.find((branch) => branch.ref === selectedRef)?.name ??
        gitReviewBranchDisplayName(selectedRef))
      : gitReviewBranchDisplayName(selectedRef)
  const normalizedQuery = query.trim().toLocaleLowerCase()
  const branches = useMemo(() => {
    if (state.status !== 'ready') return []
    if (!normalizedQuery) return state.value.branches
    return state.value.branches.filter((branch) =>
      `${branch.name}\n${branch.ref}`.toLocaleLowerCase().includes(normalizedQuery)
    )
  }, [normalizedQuery, state])

  const close = useCallback((returnFocus: boolean): void => {
    setIsOpen(false)
    setQuery('')
    if (returnFocus) window.requestAnimationFrame(() => triggerRef.current?.focus())
  }, [])

  useEffect(() => {
    if (!isOpen) return undefined
    const animationFrame = window.requestAnimationFrame(() => searchRef.current?.focus())
    return () => window.cancelAnimationFrame(animationFrame)
  }, [isOpen])

  useEffect(() => {
    if (state.status !== 'error') return
    setIsOpen(false)
  }, [state.status])

  useEffect(() => {
    if (!isActive) close(false)
  }, [close, isActive])

  useEffect(() => {
    if (!isOpen) return undefined
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target
      if (!(target instanceof Element)) return
      if (
        target.closest('.git-review__branch-menu') ||
        target.closest('.git-review__branch-picker-trigger')
      ) {
        return
      }
      close(false)
    }
    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [close, isOpen])

  const open = (): void => {
    dismissActiveTooltip()
    if (state.status === 'error') {
      onRetry()
      return
    }
    if (state.status !== 'ready') return
    setIsOpen(true)
  }

  const handleSearchKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === 'Escape') {
      event.preventDefault()
      close(true)
      return
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    event.preventDefault()
    const items = Array.from(
      event.currentTarget
        .closest('[role="menu"]')
        ?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)') ?? []
    )
    items[event.key === 'ArrowDown' ? 0 : items.length - 1]?.focus()
  }

  if (state.status === 'loading' || state.status === 'idle') {
    return (
      <span className="git-review__branch-picker-status" role="status">
        <LoaderCircle className="git-review__spinner" aria-hidden="true" />
        {t('gitReview.branch.loading')}
      </span>
    )
  }

  if (state.status === 'error') {
    return (
      <button className="git-review__branch-picker-retry" type="button" onClick={onRetry}>
        <RefreshCw aria-hidden="true" />
        {t('gitReview.retry')}
      </button>
    )
  }

  return (
    <>
      <Tooltip
        anchorClassName="git-review__branch-picker-anchor"
        content={selectedName}
        preferredPlacement="bottom"
      >
        <button
          aria-expanded={isOpen}
          aria-haspopup="menu"
          aria-label={t('gitReview.branch.select')}
          className="git-review__branch-picker-trigger"
          disabled={!isActive || state.value.branches.length === 0}
          ref={triggerRef}
          type="button"
          onClick={() => (isOpen ? close(false) : open())}
          onKeyDown={(event) => {
            if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
            event.preventDefault()
            open()
          }}
        >
          <span>{middleEllipsis(selectedName)}</span>
          <ChevronDown aria-hidden="true" />
        </button>
      </Tooltip>

      {isOpen && (
        <GitReviewMenuPortal
          anchorRef={triggerRef}
          ariaLabel={t('gitReview.branch.select')}
          className="git-review__branch-menu"
          onEscape={() => close(true)}
          placement="bottom-end"
        >
          <label className="git-review__branch-search">
            <Search aria-hidden="true" />
            <input
              aria-label={t('gitReview.branch.search')}
              placeholder={t('gitReview.branch.searchPlaceholder')}
              ref={searchRef}
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={handleSearchKeyDown}
            />
          </label>
          <div className="git-review__branch-list">
            {branches.length === 0 ? (
              <div className="git-review__menu-state" role="status">
                {t('gitReview.branch.noMatches')}
              </div>
            ) : (
              branches.map((branch) => (
                <button
                  aria-checked={branch.ref === selectedRef}
                  className="git-review__branch-item"
                  key={branch.ref}
                  role="menuitemradio"
                  title={branch.name}
                  type="button"
                  onClick={() => {
                    onSelect(branch)
                    close(true)
                  }}
                >
                  <span>{middleEllipsis(branch.name, 42)}</span>
                  {branch.ref === selectedRef && (
                    <Check className="git-review__menu-check" aria-hidden="true" />
                  )}
                </button>
              ))
            )}
          </div>
        </GitReviewMenuPortal>
      )}
    </>
  )
}

export function gitReviewBranchDisplayName(ref: string): string {
  return ref.replace(/^refs\/heads\//, '').replace(/^refs\/remotes\//, '')
}

export function middleEllipsis(value: string, maximum = 28): string {
  if (value.length <= maximum) return value
  const available = Math.max(2, maximum - 1)
  const start = Math.ceil(available / 2)
  const end = Math.floor(available / 2)
  return `${value.slice(0, start)}…${value.slice(value.length - end)}`
}
