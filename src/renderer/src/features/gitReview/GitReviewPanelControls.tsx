import { Fragment } from 'react'
import type { ReactNode, Ref } from 'react'
import type { GitReviewFile } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { Tooltip } from '../../components/overlay/Tooltip'
import { GitReviewFileIcon } from './GitReviewFileIcon'
import type { GitReviewViewMode } from './gitReviewViewMode'

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

export function ToolbarButton({
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

export function DiffLayoutIcon({ targetMode }: { targetMode: GitReviewViewMode }): ReactNode {
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

export function FileList({
  files,
  groupSources,
  onSelect,
  selectedFileId,
  t
}: FileListProps): ReactNode {
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
