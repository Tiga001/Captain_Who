import { useCallback } from 'react'
import type { RightSidebarModuleRenderProps } from '../../rightSidebar/rightSidebarTypes'
import { WorkflowsPage, type WorkflowsPageProps } from './WorkflowsPage'

type Props = Omit<WorkflowsPageProps, 'initialMonitorId' | 'onMonitorChange' | 'onTitleChange'> & {
  context: RightSidebarModuleRenderProps
}

/** A global sidebar tab: changing the active chat must not replace the workflow or its draft. */
export function WorkflowSidebarPage({ context, ...props }: Props) {
  const { page, onPageUpdate } = context
  const state = page.moduleState?.kind === 'workflows' ? page.moduleState : null
  const navigationId = state?.navigationId
  const handleMonitorChange = useCallback(
    (instanceId: string | null) => {
      onPageUpdate({ moduleState: { kind: 'workflows', instanceId, navigationId } })
    },
    [navigationId, onPageUpdate]
  )
  const handleTitleChange = useCallback((title: string) => onPageUpdate({ title }), [onPageUpdate])

  return (
    <div className="workflow-sidebar-page">
      <WorkflowsPage
        {...props}
        key={`${page.id}:${navigationId ?? 0}`}
        initialMonitorId={state?.instanceId ?? null}
        onMonitorChange={handleMonitorChange}
        onTitleChange={handleTitleChange}
      />
    </div>
  )
}
