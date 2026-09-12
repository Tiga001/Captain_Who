import { FileTree } from '@pierre/trees/react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import type { CSSProperties, FocusEvent, KeyboardEvent, PointerEvent, ReactNode } from 'react'
import {
  Check,
  ChevronRight,
  Code2,
  Copy,
  Ellipsis,
  Eye,
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
import { useWorkspaceFileRevision, useWorkspaceFileRoot } from './WorkspaceFileTreeSessions'
import { SettingsSelect } from '../settings/components/SettingsSelect'
import { isWorkspaceMarkdownFile } from './workspaceFilePreviewTypes'
import type { WorkspaceMarkdownView } from './workspaceFilePreviewTypes'
import './FilesPanel.css'

interface FilesPanelProps {
  assistantMessageId?: string
  filePath: string | null
  folderId?: string
  isActive: boolean
  markdownAnchor?: string
  markdownView: WorkspaceMarkdownView
  onMarkdownViewChange: (view: WorkspaceMarkdownView) => void
  onOpenFile: (
    path: string,
    anchor?: string,
    folderId?: string,
    assistantMessageId?: string
  ) => void
  onPdfPageChange: (page: number) => void
  onWrapLinesChange: (wrapLines: boolean) => void
  onSurfaceFocus: () => void
  pdfPage: number
  projectId: string
  projectName: string
  wrapLines: boolean
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
  assistantMessageId,
  filePath,
  folderId,
  isActive,
  markdownAnchor,
  markdownView,
  onMarkdownViewChange,
  onOpenFile,
  onPdfPageChange,
  onWrapLinesChange,
  onSurfaceFocus,
  pdfPage,
  projectId,
  projectName,
  wrapLines
}: FilesPanelProps): ReactNode {
  const { t } = useFrontendConfig()
  const treeRoot = useWorkspaceFileRoot(projectId)
  const fileFolderId = folderId ?? (assistantMessageId ? undefined : treeRoot.primaryFolderId)
  const workspaceRevision = useWorkspaceFileRevision(projectId, fileFolderId)
  const [isOptionsMenuOpen, setIsOptionsMenuOpen] = useState(false)
  const optionsControlRef = useRef<HTMLDivElement>(null)
  const optionsTriggerRef = useRef<HTMLButtonElement>(null)
  const optionItemRefs = useRef<Array<HTMLButtonElement | null>>([])
  const optionsMenuId = useId()
  const closeOptionsMenu = useCallback(() => setIsOptionsMenuOpen(false), [])
  const isMarkdown = isWorkspaceMarkdownFile(filePath)

  useDismissOnOutsidePointer(optionsControlRef, isOptionsMenuOpen, closeOptionsMenu)

  const handleFileSelect = useCallback(
    (path: string) => {
      if (treeRoot.folderId === undefined) onOpenFile(path)
      else onOpenFile(path, undefined, treeRoot.folderId)
    },
    [onOpenFile, treeRoot.folderId]
  )
  const handlePreviewOpenFile = useCallback(
    (path: string, anchor?: string) => {
      if (fileFolderId === undefined && assistantMessageId === undefined) onOpenFile(path, anchor)
      else onOpenFile(path, anchor, fileFolderId, assistantMessageId)
    },
    [assistantMessageId, fileFolderId, onOpenFile]
  )
  const handleSelectedFilePointerDown = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey) return

      const selectedFileRow = event.nativeEvent
        .composedPath()
        .find(
          (target): target is HTMLElement =>
            target instanceof HTMLElement &&
            target.dataset.itemType === 'file' &&
            target.dataset.itemSelected !== undefined
        )
      const path = selectedFileRow?.dataset.itemPath
      if (path) handleFileSelect(path)
    },
    [handleFileSelect]
  )
  const {
    failedDirectoryCount,
    hasTruncatedDirectory,
    isTreeVisible,
    model,
    refresh,
    rootState,
    searchQuery,
    setSearchQuery,
    setTreeVisible
  } = useWorkspaceFileTree({
    folderId: treeRoot.folderId,
    isActive,
    onFileSelect: handleFileSelect,
    projectId,
    selectedPath: !assistantMessageId && fileFolderId === treeRoot.folderId ? filePath : null
  })

  useEffect(() => {
    if (!isActive) closeOptionsMenu()
  }, [closeOptionsMenu, isActive])

  const revealSelectedFile = useCallback(() => {
    if (!filePath) return
    void revealWorkspaceFile({
      path: filePath,
      projectId,
      ...(fileFolderId === undefined ? {} : { folderId: fileFolderId }),
      ...(assistantMessageId === undefined ? {} : { assistantMessageId })
    }).catch(() => undefined)
  }, [assistantMessageId, fileFolderId, filePath, projectId])

  const segments = filePath?.split('/') ?? []
  const fileName = segments.at(-1) ?? null
  const ancestorPath = [projectName, ...segments.slice(0, -1)].join(' › ')
  const fullPathLabel = filePath ? `${projectName}/${filePath}` : projectName

  const focusOptionItem = (index: number): void => {
    const items = optionItemRefs.current.filter((item): item is HTMLButtonElement => item !== null)
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
      openOptionsMenu(-1)
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
    void copyWorkspaceFilePath({
      path: filePath,
      projectId,
      ...(fileFolderId === undefined ? {} : { folderId: fileFolderId }),
      ...(assistantMessageId === undefined ? {} : { assistantMessageId })
    }).catch(() => undefined)
  }

  const wrapLinesOptionIndex = isMarkdown ? 2 : 0
  const copyPathOptionIndex = isMarkdown ? 3 : 1

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
            {isOptionsMenuOpen && filePath && (
              <div className="files-panel__options-menu" id={optionsMenuId} role="menu">
                {isMarkdown && (
                  <>
                    <button
                      type="button"
                      role="menuitemradio"
                      aria-checked={markdownView === 'source'}
                      onClick={() => {
                        onMarkdownViewChange('source')
                        closeOptionsMenuAndRestoreFocus()
                      }}
                      onKeyDown={(event) => handleOptionItemKeyDown(event, 0)}
                      ref={(node) => {
                        optionItemRefs.current[0] = node
                      }}
                    >
                      <Code2 aria-hidden="true" />
                      <span>{t('files.markdown.source')}</span>
                      {markdownView === 'source' && (
                        <Check className="files-panel__menu-check" aria-hidden="true" />
                      )}
                    </button>
                    <button
                      type="button"
                      role="menuitemradio"
                      aria-checked={markdownView === 'preview'}
                      onClick={() => {
                        onMarkdownViewChange('preview')
                        closeOptionsMenuAndRestoreFocus()
                      }}
                      onKeyDown={(event) => handleOptionItemKeyDown(event, 1)}
                      ref={(node) => {
                        optionItemRefs.current[1] = node
                      }}
                    >
                      <Eye aria-hidden="true" />
                      <span>{t('files.markdown.preview')}</span>
                      {markdownView === 'preview' && (
                        <Check className="files-panel__menu-check" aria-hidden="true" />
                      )}
                    </button>
                    <div className="files-panel__menu-separator" role="separator" />
                  </>
                )}
                <button
                  type="button"
                  role="menuitemcheckbox"
                  aria-checked={wrapLines}
                  onClick={() => {
                    onWrapLinesChange(!wrapLines)
                    closeOptionsMenuAndRestoreFocus()
                  }}
                  onKeyDown={(event) => handleOptionItemKeyDown(event, wrapLinesOptionIndex)}
                  ref={(node) => {
                    optionItemRefs.current[wrapLinesOptionIndex] = node
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
                  onKeyDown={(event) => handleOptionItemKeyDown(event, copyPathOptionIndex)}
                  ref={(node) => {
                    optionItemRefs.current[copyPathOptionIndex] = node
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
              onClick={() => setTreeVisible(!isTreeVisible)}
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
            key={JSON.stringify([
              projectId,
              fileFolderId,
              assistantMessageId,
              assistantMessageId ? null : workspaceRevision
            ])}
            assistantMessageId={assistantMessageId}
            folderId={fileFolderId}
            isActive={isActive}
            markdownAnchor={markdownAnchor}
            markdownView={markdownView}
            onOpenFile={handlePreviewOpenFile}
            onPdfPageChange={onPdfPageChange}
            path={filePath}
            pdfPage={pdfPage}
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
          {treeRoot.folders.length > 1 && (
            <div className="files-panel__root-row">
              <SettingsSelect
                ariaLabel={t('files.selectRoot')}
                className="files-panel__root-select"
                disabled={!isActive || !isTreeVisible}
                onChange={treeRoot.selectFolder}
                options={treeRoot.folders.map((folder) => ({
                  value: folder.id,
                  label: folder.alias
                }))}
                value={treeRoot.folderId ?? ''}
              />
            </div>
          )}
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

          <div
            className="files-panel__tree-content"
            onPointerDownCapture={handleSelectedFilePointerDown}
          >
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
