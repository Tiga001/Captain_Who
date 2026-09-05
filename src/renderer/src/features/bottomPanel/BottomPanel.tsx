import { memo, useCallback, useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import { RightSidebarPageStack } from '../rightSidebar/RightSidebarPageStack'
import { RightSidebarTabStrip } from '../rightSidebar/RightSidebarTabStrip'
import { useRightSidebarDocumentVisibility } from '../rightSidebar/rightSidebarActivity'
import { useRightSidebarModules } from '../rightSidebar/useRightSidebarModules'
import { useRightSidebarPlatform } from '../rightSidebar/useRightSidebarPlatform'
import type {
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarPageOpenRequest
} from '../rightSidebar/rightSidebarTypes'
import '../rightSidebar/RightSidebar.css'
import './BottomPanel.css'

interface BottomPanelProps {
  activeConversationId?: string | null
  capabilities?: RightSidebarCapabilities
  collaborationSnapshot?: CollaborationStoreSnapshot | null
  isOpen: boolean
  isWorkspaceVisible: boolean
  modules?: RightSidebarModuleDefinition[]
  onClose: () => void
  onOpenRightModule: (moduleId: RightSidebarModuleId) => void
  workspaceKey?: string | null
  workspaceKeys?: readonly string[]
  workspaceName?: string | null
  workspacePath?: string
}

export const BottomPanel = memo(function BottomPanel({
  activeConversationId,
  capabilities,
  collaborationSnapshot,
  isOpen,
  isWorkspaceVisible,
  modules: configuredModules,
  onClose,
  onOpenRightModule,
  workspaceKey,
  workspaceKeys,
  workspaceName,
  workspacePath
}: BottomPanelProps) {
  const { t } = useFrontendConfig()
  const documentVisible = useRightSidebarDocumentVisibility()
  const visible = isOpen && isWorkspaceVisible
  const [isMenuOpen, setIsMenuOpen] = useState(false)
  const openedRef = useRef(false)
  const { modules } = useRightSidebarModules({
    activeConversationId,
    collaborationSnapshot,
    configuredModules
  })
  const {
    activatePage,
    activePageId,
    availableModules,
    closePage,
    moduleAvailability,
    openModule,
    pages,
    updatePage
  } = useRightSidebarPlatform({
    capabilities,
    modules,
    t,
    workspaceKey,
    workspaceKeys,
    workspaceName,
    workspacePath
  })

  useEffect(() => {
    if (!visible) {
      openedRef.current = false
      setIsMenuOpen(false)
      return
    }
    if (openedRef.current) return
    openedRef.current = true
    if (pages.length === 0) openModule('terminal')
  }, [openModule, pages.length, visible])

  const closeMenu = useCallback(() => setIsMenuOpen(false), [])
  const handleOpenModule = useCallback(
    (moduleId: RightSidebarModuleId) => {
      closeMenu()
      if (moduleId === 'terminal') openModule('terminal')
      else onOpenRightModule(moduleId)
    },
    [closeMenu, onOpenRightModule, openModule]
  )
  const handleClosePage = useCallback(
    (pageId: string) => {
      if (pages.length === 1 && pages[0].id === pageId) onClose()
      closePage(pageId)
    },
    [closePage, onClose, pages]
  )
  const handleOpenRelatedPage = useCallback(
    (
      _sourcePageId: string,
      module: RightSidebarModuleDefinition,
      request: RightSidebarPageOpenRequest
    ) => handleOpenModule(request.targetModuleId ?? module.id),
    [handleOpenModule]
  )

  return (
    <section className="bottom-panel" aria-label={t('app.bottomPanel')}>
      <header className="bottom-panel__toolbar right-sidebar__toolbar">
        <RightSidebarTabStrip
          activePageId={activePageId}
          availableModules={availableModules}
          dragRegion={false}
          isMenuOpen={isMenuOpen}
          menuPlacement="top"
          modules={modules}
          onActivatePage={activatePage}
          onClosePage={handleClosePage}
          onMenuOpenChange={setIsMenuOpen}
          onOpenModule={handleOpenModule}
          pages={pages}
          t={t}
        />
        <button
          className="right-sidebar__icon-button"
          type="button"
          aria-label={t('app.collapseBottomPanel')}
          title={t('app.collapseBottomPanel')}
          onClick={onClose}
        >
          <X aria-hidden="true" />
        </button>
      </header>
      <div className="right-sidebar__content">
        <RightSidebarPageStack
          activePageId={activePageId}
          availability={moduleAvailability}
          documentVisible={documentVisible}
          modules={modules}
          onOpenPage={handleOpenRelatedPage}
          onPageUpdate={updatePage}
          onSurfaceFocus={closeMenu}
          pages={pages}
          sidebarVisible={visible}
          t={t}
        />
      </div>
    </section>
  )
})
