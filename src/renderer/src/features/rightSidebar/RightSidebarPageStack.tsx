import { memo, useCallback } from 'react'
import type { Translate } from '../../config/translationFormat'
import { resolveRightSidebarActivity } from './rightSidebarActivity'
import {
  getRightSidebarModuleAvailability,
  resolveRightSidebarPageAvailability
} from './rightSidebarModuleAvailability'
import type {
  RightSidebarActivity,
  RightSidebarModuleAvailability,
  RightSidebarModuleAvailabilityMap,
  RightSidebarModuleDefinition,
  RightSidebarPage,
  RightSidebarPageOpenRequest,
  RightSidebarPageUpdate
} from './rightSidebarTypes'

interface RightSidebarPageStackProps {
  activePageId: string | null
  automationPageIds?: ReadonlySet<string>
  availability: RightSidebarModuleAvailabilityMap
  documentVisible: boolean
  modules: RightSidebarModuleDefinition[]
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onOpenPage: (
    sourcePageId: string,
    module: RightSidebarModuleDefinition,
    request: RightSidebarPageOpenRequest
  ) => void
  onSurfaceFocus: () => void
  pages: RightSidebarPage[]
  sidebarVisible: boolean
  t: Translate
}

export const RightSidebarPageStack = memo(function RightSidebarPageStack({
  activePageId,
  automationPageIds,
  availability,
  documentVisible,
  modules,
  onOpenPage,
  onPageUpdate,
  onSurfaceFocus,
  pages,
  sidebarVisible,
  t
}: RightSidebarPageStackProps) {
  return (
    <div className="right-sidebar__page-stack">
      {pages.map((page) => {
        const module = modules.find((candidate) => candidate.id === page.moduleId)
        if (!module) return null

        const isSelected = page.id === activePageId
        const activity = resolveRightSidebarActivity({
          documentVisible,
          isSelected,
          sidebarVisible
        })
        const pageAvailability = resolveRightSidebarPageAvailability(
          module,
          getRightSidebarModuleAvailability(availability, module.id),
          page
        )

        return (
          <RightSidebarPageFrame
            activity={activity}
            automationActive={automationPageIds?.has(page.id) === true}
            availability={pageAvailability}
            isSelected={isSelected}
            key={`${page.id}:${page.workspaceSessionKey ?? 'global'}`}
            module={module}
            onOpenPage={onOpenPage}
            onPageUpdate={onPageUpdate}
            onSurfaceFocus={onSurfaceFocus}
            page={page}
            t={t}
          />
        )
      })}
    </div>
  )
})

interface RightSidebarPageFrameProps {
  activity: RightSidebarActivity
  automationActive: boolean
  availability: RightSidebarModuleAvailability
  isSelected: boolean
  module: RightSidebarModuleDefinition
  onOpenPage: (
    sourcePageId: string,
    module: RightSidebarModuleDefinition,
    request: RightSidebarPageOpenRequest
  ) => void
  onPageUpdate: (pageId: string, update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

const RightSidebarPageFrame = memo(function RightSidebarPageFrame({
  activity,
  automationActive,
  availability,
  isSelected,
  module,
  onOpenPage,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarPageFrameProps) {
  const updatePage = useCallback(
    (update: RightSidebarPageUpdate) => onPageUpdate(page.id, update),
    [onPageUpdate, page.id]
  )
  const openPage = useCallback(
    (request: RightSidebarPageOpenRequest) => onOpenPage(page.id, module, request),
    [module, onOpenPage, page.id]
  )

  const shouldMount = isSelected || module.retention === 'keep-alive'

  return (
    <section
      className="right-sidebar__page"
      data-active={isSelected ? 'true' : undefined}
      data-agent-rendering={
        automationActive && module.surfaceKind === 'webview' ? 'true' : undefined
      }
      data-activity={activity}
      data-surface-kind={module.surfaceKind}
      aria-hidden={isSelected ? undefined : true}
      inert={automationActive && !isSelected ? true : undefined}
    >
      {shouldMount &&
        module.render({
          activity,
          availability,
          isSelected,
          onOpenPage: openPage,
          onPageUpdate: updatePage,
          onSurfaceFocus,
          page,
          t
        })}
    </section>
  )
})
