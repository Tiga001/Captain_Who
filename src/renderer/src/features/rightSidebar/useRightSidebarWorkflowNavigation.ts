import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import type {
  RightSidebarModuleId,
  RightSidebarModulePageState,
  RightSidebarPage,
  RightSidebarWorkflowNavigationRequest
} from './rightSidebarTypes'

/** Workflow drafts survive tab/context changes; only replacing or closing their page is guarded. */
export function useRightSidebarWorkflowNavigation({
  enabled,
  pages,
  request,
  onBeforeNavigate,
  openModule,
  closePage
}: {
  enabled: boolean
  pages: readonly RightSidebarPage[]
  request?: RightSidebarWorkflowNavigationRequest | null
  onBeforeNavigate?: (proceed: () => void) => void
  openModule: (id: RightSidebarModuleId, state?: RightSidebarModulePageState) => void
  closePage: (id: string) => void
}) {
  const handledRequestIdRef = useRef<number | null>(null)
  const generationRef = useRef(0)
  const mountedRef = useRef(true)
  const currentRef = useRef({ enabled, pages, onBeforeNavigate, openModule, closePage })
  useLayoutEffect(() => {
    currentRef.current = { enabled, pages, onBeforeNavigate, openModule, closePage }
  }, [enabled, pages, onBeforeNavigate, openModule, closePage])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      generationRef.current += 1
    }
  }, [])

  const changeWorkflowPage = useCallback((instanceId: string | null, closingPageId?: string) => {
    const current = currentRef.current
    if (!current.enabled) return
    const existing = current.pages.find((page) => page.moduleId === 'workflows')
    if (closingPageId && existing?.id !== closingPageId) return
    const generation = ++generationRef.current
    const existingNavigationId =
      existing?.moduleState?.kind === 'workflows' ? existing.moduleState.navigationId : undefined
    const proceed = () => {
      if (!mountedRef.current || generationRef.current !== generation) return
      const latest = currentRef.current
      if (!latest.enabled) return
      const latestPage = latest.pages.find((page) => page.moduleId === 'workflows')
      const latestNavigationId =
        latestPage?.moduleState?.kind === 'workflows'
          ? latestPage.moduleState.navigationId
          : undefined
      // A delayed confirmation must never discard a replacement tab or a newer navigation.
      if (latestPage?.id !== existing?.id || latestNavigationId !== existingNavigationId) return
      generationRef.current += 1
      if (closingPageId) {
        latest.closePage(closingPageId)
      } else {
        latest.openModule('workflows', {
          kind: 'workflows',
          instanceId,
          navigationId: generation
        })
      }
    }
    if (existing && current.onBeforeNavigate) current.onBeforeNavigate(proceed)
    else proceed()
  }, [])

  useEffect(() => {
    if (
      !enabled ||
      !request ||
      (handledRequestIdRef.current !== null && request.requestId <= handledRequestIdRef.current)
    ) {
      return
    }
    handledRequestIdRef.current = request.requestId
    changeWorkflowPage(request.instanceId)
  }, [changeWorkflowPage, enabled, request])

  return {
    openWorkflow: changeWorkflowPage,
    closeWorkflow: (pageId: string) => changeWorkflowPage(null, pageId)
  }
}
