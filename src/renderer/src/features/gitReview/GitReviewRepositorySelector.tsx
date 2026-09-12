import { useRef } from 'react'
import { Check, ChevronDown } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { GitReviewMenuPortal } from './GitReviewMenuPortal'

interface Props {
  folders: readonly { folderId: string; alias: string }[]
  selectedFolderId?: string
  allSelected: boolean
  allowAll: boolean
  isOpen: boolean
  onOpenChange: (open: boolean) => void
  onSelect: (folderId: string | null) => void
  t: Translate
}

export function GitReviewRepositorySelector({
  folders,
  selectedFolderId,
  allSelected,
  allowAll,
  isOpen,
  onOpenChange,
  onSelect,
  t
}: Props) {
  const anchorRef = useRef<HTMLButtonElement>(null)
  const selected = folders.find((folder) => folder.folderId === selectedFolderId)
  const label = allSelected
    ? t('gitReview.repositories.all')
    : (selected?.alias ?? t('gitReview.repositories.select'))
  const close = () => {
    onOpenChange(false)
    requestAnimationFrame(() => anchorRef.current?.focus())
  }
  const select = (folderId: string | null) => {
    onSelect(folderId)
    close()
  }
  return (
    <div className="git-review__repository-control">
      <button
        ref={anchorRef}
        type="button"
        className="git-review__scope-button"
        aria-label={t('gitReview.repositories.select')}
        aria-haspopup="menu"
        aria-expanded={isOpen}
        onClick={() => onOpenChange(!isOpen)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault()
            onOpenChange(true)
          }
        }}
      >
        <span className="git-review__scope-label">{label}</span>
        <ChevronDown aria-hidden="true" />
      </button>
      {isOpen && (
        <GitReviewMenuPortal
          anchorRef={anchorRef}
          ariaLabel={t('gitReview.repositories.select')}
          autoFocus="selected"
          className="git-review__source-menu git-review__repository-menu"
          onEscape={close}
          placement="bottom-start"
        >
          {allowAll && (
            <button
              type="button"
              role="menuitemradio"
              aria-checked={allSelected}
              className="git-review__source-menu-item"
              onClick={() => select(null)}
            >
              <span>{t('gitReview.repositories.all')}</span>
              {allSelected && <Check aria-hidden="true" />}
            </button>
          )}
          {folders.map((folder) => (
            <button
              key={folder.folderId}
              type="button"
              role="menuitemradio"
              aria-checked={!allSelected && selectedFolderId === folder.folderId}
              className="git-review__source-menu-item"
              onClick={() => select(folder.folderId)}
            >
              <span>{folder.alias}</span>
              {!allSelected && selectedFolderId === folder.folderId && <Check aria-hidden="true" />}
            </button>
          ))}
        </GitReviewMenuPortal>
      )}
    </div>
  )
}
