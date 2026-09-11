import { ArrowUpRight, Folder, MessageCircle, PencilLine } from 'lucide-react'
import { useRef } from 'react'
import { AnchoredPopover } from '../../components/overlay/AnchoredPopover'
import {
  formatProjectFolderPath,
  sortedProjectFolders,
  type AppProject
} from '../../config/projectConfig'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import './MainPanelProjectCard.css'

export interface MainPanelProjectCardProps {
  conversationCount: number
  onEditProject: () => void
  onOpenChange: (open: boolean) => void
  onRevealFolder: (folderId: string) => void
  open: boolean
  project: AppProject
  t: Translate
}

export function MainPanelProjectCard({
  conversationCount,
  onEditProject,
  onOpenChange,
  onRevealFolder,
  open,
  project,
  t
}: MainPanelProjectCardProps) {
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const folders = sortedProjectFolders(project)
  const close = () => onOpenChange(false)
  useDismissOnOutsidePointer(rootRef, open, close, (target) =>
    Boolean(popoverRef.current?.contains(target))
  )

  return (
    <div className="main-panel__project" ref={rootRef}>
      <button
        className="main-panel__project-button"
        type="button"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={t('project.openDetails')}
        data-open={open || undefined}
        onClick={() => onOpenChange(!open)}
        ref={triggerRef}
      >
        <Folder aria-hidden="true" />
      </button>
      {open ? (
        <AnchoredPopover
          align="start"
          anchorRef={triggerRef}
          className="main-panel__project-popover"
          enabled
          onClose={close}
          popoverRef={popoverRef}
        >
          <div className="main-panel__project-card" role="dialog" aria-label={project.name}>
            <div className="main-panel__project-card-header">
              <Folder aria-hidden="true" />
              <span>{project.name}</span>
            </div>
            <div className="main-panel__project-card-count">
              <MessageCircle aria-hidden="true" />
              <span>
                {formatTranslation(t, 'project.conversationCount', { count: conversationCount })}
              </span>
            </div>
            {folders.length > 0 ? (
              <div className="main-panel__project-card-folders">
                {folders.map((folder) => {
                  const displayPath = formatProjectFolderPath(folder.path)
                  return (
                    <button
                      className="main-panel__project-card-folder"
                      type="button"
                      key={folder.id}
                      title={folder.path}
                      aria-label={`${t('project.showInFolder')}: ${displayPath}`}
                      onClick={() => {
                        onRevealFolder(folder.id)
                        close()
                      }}
                    >
                      <Folder aria-hidden="true" />
                      <span>{displayPath}</span>
                      <ArrowUpRight aria-hidden="true" />
                    </button>
                  )
                })}
              </div>
            ) : null}
            <button
              className="main-panel__project-card-edit"
              type="button"
              onClick={() => {
                close()
                onEditProject()
              }}
            >
              <PencilLine aria-hidden="true" />
              <span>{t('project.editProject')}</span>
            </button>
          </div>
        </AnchoredPopover>
      ) : null}
    </div>
  )
}
