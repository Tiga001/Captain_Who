import { useCallback } from 'react'
import type { Translate } from '../../config/translationFormat'
import { getRightSidebarModuleAvailability } from './rightSidebarModuleAvailability'
import type {
  RightSidebarModuleAvailabilityMap,
  RightSidebarModuleDefinition,
  RightSidebarPage,
  RightSidebarPageOpenRequest,
  RightSidebarPageUpdate
} from './rightSidebarTypes'

interface RightSidebarPageStackProps {
  activePageId: string | null
  availability: RightSidebarModuleAvailabilityMap
  modules: RightSidebarModuleDefinition[]
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onOpenPage: (sourcePageId: string, request: RightSidebarPageOpenRequest) => void
  onSurfaceFocus: () => void
  pages: RightSidebarPage[]
  t: Translate
}

export function RightSidebarPageStack({
  activePageId,
  availability,
  modules,
  onOpenPage,
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
          availability={availability}
          key={`${page.id}:${page.workspaceSessionKey ?? 'global'}`}
          modules={modules}
          onOpenPage={onOpenPage}
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
  availability: RightSidebarModuleAvailabilityMap
  modules: RightSidebarModuleDefinition[]
  onOpenPage: (sourcePageId: string, request: RightSidebarPageOpenRequest) => void
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

function RightSidebarPageFrame({
  activePageId,
  availability,
  modules,
  onOpenPage,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarPageFrameProps) {
  const module = modules.find((candidate) => candidate.id === page.moduleId)
  const isActive = page.id === activePageId
  const updatePage = useCallback(
    (update: RightSidebarPageUpdate) => onPageUpdate(page.id, update),
    [onPageUpdate, page.id]
  )
  const openPage = useCallback(
    (request: RightSidebarPageOpenRequest) => onOpenPage(page.id, request),
    [onOpenPage, page.id]
  )

  if (!module) return null

  const shouldMount = isActive || module.retention === 'keep-alive'
  const moduleAvailability = getRightSidebarModuleAvailability(availability, module.id)

  return (
    <section
      className="right-sidebar__page"
      data-active={isActive ? 'true' : undefined}
      data-surface-kind={module.surfaceKind}
      aria-hidden={isActive ? undefined : true}
    >
      {shouldMount &&
        module.render({
          availability: moduleAvailability,
          isActive,
          onOpenPage: openPage,
          onPageUpdate: updatePage,
          onSurfaceFocus,
          page,
          t
        })}
    </section>
  )
}
