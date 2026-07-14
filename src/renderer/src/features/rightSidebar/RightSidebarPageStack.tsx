import { useCallback } from 'react'
import type { Translate } from '../../config/translationFormat'
import { getRightSidebarModule } from './rightSidebarModules'
import type { RightSidebarPage, RightSidebarPageUpdate } from './rightSidebarTypes'

interface RightSidebarPageStackProps {
  activePageId: string | null
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  pages: RightSidebarPage[]
  t: Translate
}

export function RightSidebarPageStack({
  activePageId,
  onPageUpdate,
  onSurfaceFocus,
  pages,
  t
}: RightSidebarPageStackProps) {
  return (
    <div className="right-sidebar__page-stack">
      {pages.map((page) => (
        <RightSidebarPageFrame
          activePageId={activePageId}
          key={page.id}
          onPageUpdate={onPageUpdate}
          onSurfaceFocus={onSurfaceFocus}
          page={page}
          t={t}
        />
      ))}
    </div>
  )
}

interface RightSidebarPageFrameProps {
  activePageId: string | null
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

function RightSidebarPageFrame({
  activePageId,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarPageFrameProps) {
  const module = getRightSidebarModule(page.moduleId)
  const isActive = page.id === activePageId
  const updatePage = useCallback(
    (update: RightSidebarPageUpdate) => onPageUpdate(page.id, update),
    [onPageUpdate, page.id]
  )

  if (!module) return null

  const shouldMount = isActive || module.retention === 'keep-alive'

  return (
    <section
      className="right-sidebar__page"
      data-active={isActive ? 'true' : undefined}
      data-surface-kind={module.surfaceKind}
      aria-hidden={isActive ? undefined : true}
    >
      {shouldMount &&
        module.render({
          isActive,
          onPageUpdate: updatePage,
          onSurfaceFocus,
          page,
          t
        })}
    </section>
  )
}
