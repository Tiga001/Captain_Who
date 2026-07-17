import { FileTree } from '@pierre/trees/react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import type { CSSProperties, FocusEvent, KeyboardEvent, ReactNode } from 'react'
import {
  Check,
  ChevronRight,
  Copy,
  Ellipsis,
  FolderOpen,
  PanelRight,
  RefreshCw,
  Search,
  WrapText,
  X
} from 'lucide-react'
import { Tooltip } from '../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import { copyWorkspaceFilePath, revealWorkspaceFile } from './filesClient'
import { useWorkspaceFileTree } from './useWorkspaceFileTree'
import { WorkspaceFilePreview } from './WorkspaceFilePreview'
import './FilesPanel.css'

interface FilesPanelProps {
  filePath: string | null
  isActive: boolean
  onOpenFile: (path: string) => void
  onSurfaceFocus: () => void
  projectId: string
  projectName: string
}

const TREE_STYLE = {
  '--trees-accent-override': 'var(--mc-color-text-accent)',
  '--trees-bg-muted-override': 'var(--mc-color-state-hover)',
  '--trees-bg-override': 'var(--mc-color-surface-right-panel)',
  '--trees-border-color-override': 'var(--mc-color-border-default)',
  '--trees-fg-muted-override': 'var(--mc-color-icon-muted)',
  '--trees-fg-override': 'var(--mc-color-text-primary)',
  '--trees-focus-ring-color-override': 'var(--mc-color-focus-ring)',
  '--trees-font-family-override': 'var(--mc-font-family)',
  '--trees-font-size-override': '13px',
  '--trees-item-margin-x-override': '3px',
  '--trees-padding-inline-override': '8px',
  '--trees-selected-bg-override': 'var(--mc-color-state-active)',
  height: '100%'
} as CSSProperties

export function FilesPanel({
  filePath,
  isActive,
  onOpenFile,
  onSurfaceFocus,
  projectId,
  projectName
}: FilesPanelProps): ReactNode {
  const { t } = useFrontendConfig()
  const [isTreeVisible, setIsTreeVisible] = useState(true)
  const [isOptionsMenuOpen, setIsOptionsMenuOpen] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [wrapLines, setWrapLines] = useState(false)
  const optionsControlRef = useRef<HTMLDivElement>(null)
  const optionsTriggerRef = useRef<HTMLButtonElement>(null)
  const optionItemRefs = useRef<Array<HTMLButtonElement | null>>([])
  const optionsMenuId = useId()
  const closeOptionsMenu = useCallback(() => setIsOptionsMenuOpen(false), [])

  useDismissOnOutsidePointer(optionsControlRef, isOptionsMenuOpen, closeOptionsMenu)

  const handleFileSelect = useCallback(
    (path: string) => {
      onOpenFile(path)
    },
    [onOpenFile]
  )
  const { failedDirectoryCount, hasTruncatedDirectory, model, refresh, rootState } =
    useWorkspaceFileTree({
      isActive,
      onFileSelect: handleFileSelect,
      projectId,
      selectedPath: filePath
    })

  useEffect(() => {
    model.setSearch(searchQuery.trim() || null)
  }, [model, searchQuery])

  useEffect(() => {
    if (!isActive) closeOptionsMenu()
  }, [closeOptionsMenu, isActive])

  const revealSelectedFile = useCallback(() => {
    if (!filePath) return
    void revealWorkspaceFile({ path: filePath, projectId }).catch(() => undefined)
  }, [filePath, projectId])

  const segments = filePath?.split('/') ?? []
  const fileName = segments.at(-1) ?? null
  const ancestorPath = [projectName, ...segments.slice(0, -1)].join(' › ')
  const fullPathLabel = filePath ? `${projectName}/${filePath}` : projectName

  const focusOptionItem = (index: number): void => {
    const items = optionItemRefs.current
    if (items.length === 0) return
    items[(index + items.length) % items.length]?.focus()
  }

  const openOptionsMenu = (focusIndex = 0): void => {
    setIsOptionsMenuOpen(true)
    window.requestAnimationFrame(() => focusOptionItem(focusIndex))
  }

  const closeOptionsMenuAndRestoreFocus = (): void => {
    closeOptionsMenu()
    window.requestAnimationFrame(() => optionsTriggerRef.current?.focus())
  }

  const handleOptionsBlur = (event: FocusEvent<HTMLDivElement>): void => {
    if (!event.currentTarget.contains(event.relatedTarget)) closeOptionsMenu()
  }

  const handleOptionsTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>): void => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      openOptionsMenu(0)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      openOptionsMenu(optionItemRefs.current.length - 1)
    } else if (event.key === 'Escape' && isOptionsMenuOpen) {
      event.preventDefault()
      closeOptionsMenu()
    }
  }

  const handleOptionItemKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    index: number
  ): void => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      focusOptionItem(index + 1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      focusOptionItem(index - 1)
    } else if (event.key === 'Home') {
      event.preventDefault()
      focusOptionItem(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      focusOptionItem(optionItemRefs.current.length - 1)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      closeOptionsMenuAndRestoreFocus()
    }
  }

  const copySelectedFilePath = (): void => {
    if (!filePath) return
    closeOptionsMenuAndRestoreFocus()
    void copyWorkspaceFilePath({ path: filePath, projectId }).catch(() => undefined)
  }

  return (
    <div className="files-panel" onPointerDown={onSurfaceFocus}>
      <header className="files-panel__toolbar">
        <div
          className="files-panel__breadcrumbs"
          aria-label={`${t('files.breadcrumbs')}: ${fullPathLabel}`}
          title={fullPathLabel}
        >
          {fileName ? (
            <>
              <span className="files-panel__breadcrumb-prefix">
                <bdi className="files-panel__breadcrumb-prefix-text" dir="ltr">
                  {ancestorPath}
                </bdi>
              </span>
              <ChevronRight className="files-panel__breadcrumb-separator" aria-hidden="true" />
              <strong className="files-panel__breadcrumb-filename">{fileName}</strong>
            </>
          ) : (
            <strong className="files-panel__breadcrumb-root">{projectName}</strong>
          )}
        </div>
        <div className="files-panel__toolbar-actions">
          <div
            className="files-panel__options-control"
            onBlur={handleOptionsBlur}
            ref={optionsControlRef}
          >
            <Tooltip content={t('files.options')} preferredPlacement="bottom">
              <button
                className="files-panel__icon-button"
                type="button"
                aria-controls={isOptionsMenuOpen ? optionsMenuId : undefined}
                aria-expanded={isOptionsMenuOpen}
                aria-haspopup="menu"
                aria-label={t('files.options')}
                data-active={isOptionsMenuOpen ? 'true' : undefined}
                disabled={!filePath}
                onClick={() => (isOptionsMenuOpen ? closeOptionsMenu() : openOptionsMenu())}
                onKeyDown={handleOptionsTriggerKeyDown}
                ref={optionsTriggerRef}
              >
                <Ellipsis aria-hidden="true" />
              </button>
            </Tooltip>
            {isOptionsMenuOpen && filePath && (
              <div className="files-panel__options-menu" id={optionsMenuId} role="menu">
                <button
                  type="button"
                  role="menuitemcheckbox"
                  aria-checked={wrapLines}
                  onClick={() => {
                    setWrapLines((value) => !value)
                    closeOptionsMenuAndRestoreFocus()
                  }}
                  onKeyDown={(event) => handleOptionItemKeyDown(event, 0)}
                  ref={(node) => {
                    optionItemRefs.current[0] = node
                  }}
                >
                  <WrapText aria-hidden="true" />
                  <span>{t('files.wrapLines')}</span>
                  {wrapLines && <Check className="files-panel__menu-check" aria-hidden="true" />}
                </button>
                <button
                  type="button"
                  role="menuitem"
                  onClick={copySelectedFilePath}
                  onKeyDown={(event) => handleOptionItemKeyDown(event, 1)}
                  ref={(node) => {
                    optionItemRefs.current[1] = node
                  }}
                >
                  <Copy aria-hidden="true" />
                  <span>{t('files.copyPath')}</span>
                </button>
              </div>
            )}
          </div>
          <Tooltip content={t('files.reveal')}>
            <button
              className="files-panel__icon-button"
              type="button"
              aria-label={t('files.reveal')}
              disabled={!filePath}
              onClick={revealSelectedFile}
            >
              <FolderOpen aria-hidden="true" />
            </button>
          </Tooltip>
          <Tooltip content={isTreeVisible ? t('files.hideTree') : t('files.showTree')}>
            <button
              className="files-panel__icon-button"
              type="button"
              aria-label={isTreeVisible ? t('files.hideTree') : t('files.showTree')}
              aria-pressed={isTreeVisible}
              data-active={isTreeVisible ? 'true' : undefined}
              onClick={() => setIsTreeVisible((value) => !value)}
            >
              <PanelRight aria-hidden="true" />
            </button>
          </Tooltip>
        </div>
      </header>

      <div
        className="files-panel__workspace"
        data-tree-visible={isTreeVisible ? 'true' : undefined}
      >
        <main className="files-panel__preview">
          <WorkspaceFilePreview
            isActive={isActive}
            path={filePath}
            projectId={projectId}
            wrapLines={wrapLines}
          />
        </main>

        <aside
          className="files-panel__tree-pane"
          aria-hidden={!isTreeVisible}
          aria-label={t('files.tree')}
          data-visible={isTreeVisible ? 'true' : undefined}
          inert={isTreeVisible ? undefined : true}
        >
          <div className="files-panel__tree-search">
            <Search aria-hidden="true" />
            <input
              type="text"
              aria-label={t('files.filter')}
              placeholder={t('files.filterPlaceholder')}
              value={searchQuery}
              onChange={(event) => setSearchQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === 'Escape') setSearchQuery('')
              }}
            />
            {searchQuery ? (
              <button
                type="button"
                aria-label={t('files.clearFilter')}
                onClick={() => setSearchQuery('')}
              >
                <X aria-hidden="true" />
              </button>
            ) : (
              <Tooltip content={t('files.refresh')}>
                <button type="button" aria-label={t('files.refresh')} onClick={refresh}>
                  <RefreshCw aria-hidden="true" />
                </button>
              </Tooltip>
            )}
          </div>

          <div className="files-panel__tree-content">
            <FileTree className="files-panel__tree" model={model} style={TREE_STYLE} />
            {rootState?.status === 'loading' && (
              <div className="files-panel__tree-overlay">{t('files.loading')}</div>
            )}
            {rootState?.status === 'error' && (
              <div className="files-panel__tree-overlay">
                <span>{t('files.loadError')}</span>
                <button type="button" onClick={refresh}>
                  {t('files.retry')}
                </button>
              </div>
            )}
            {rootState?.status === 'ready' && rootState.value.entries.length === 0 && (
              <div className="files-panel__tree-overlay">{t('files.emptyProject')}</div>
            )}
          </div>

          {(failedDirectoryCount > 0 || hasTruncatedDirectory) && (
            <div className="files-panel__tree-notice">
              {failedDirectoryCount > 0 ? t('files.partialError') : t('files.truncated')}
            </div>
          )}
        </aside>
      </div>
    </div>
  )
}
