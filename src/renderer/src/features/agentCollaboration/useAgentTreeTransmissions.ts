import type { AgentSummary } from '@mycopilot/protocol'
import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import {
  routeAgentTreeTransmission,
  type AgentTreeTransmission,
  type AgentTreeTransmissionRoute,
  type TreeNodeBounds
} from './agentTreeTransmission'
import type { AgentTreeLayout } from './AgentTreeView'

const MAX_ACTIVE = 3
const MAX_QUEUED = 9
const MAX_AGE_MS = 4000

interface VisibleTransmission extends AgentTreeTransmission {
  route: AgentTreeTransmissionRoute
  startedAt: number
}

interface Options {
  layout: AgentTreeLayout
  transmissions: readonly AgentTreeTransmission[]
  agents: readonly AgentSummary[]
  collapsedAgentIds: ReadonlySet<string>
  userCollapsed: boolean
  scrollRef: RefObject<HTMLDivElement | null>
}

export function useAgentTreeTransmissions({
  layout,
  transmissions,
  agents,
  collapsedAgentIds,
  userCollapsed,
  scrollRef
}: Options) {
  const environment = useRef({ layout, transmissions, agents, scrollRef })
  const seen = useRef(new Set(transmissions.map((event) => event.id)))
  const queued = useRef<AgentTreeTransmission[]>([])
  const active = useRef<VisibleTransmission[]>([])
  const reducedMotion = useRef(false)
  const [view, setView] = useState<{
    width: number
    height: number
    active: VisibleTransmission[]
  }>({ width: 0, height: 0, active: [] })

  const refresh = useCallback(() => {
    const { layout, transmissions, agents, scrollRef } = environment.current
    const element = scrollRef.current
    const root = element?.querySelector<HTMLElement>('.agent-tree__roots')
    const parentById = new Map(agents.map((agent) => [agent.agentId, agent.parentAgentId]))
    const origin = element?.getBoundingClientRect()
    const enabled =
      layout === 'diagram' &&
      !reducedMotion.current &&
      document.visibilityState !== 'hidden' &&
      element &&
      root &&
      origin &&
      origin.width > 0 &&
      origin.height > 0 &&
      element.getClientRects().length > 0 &&
      !element.closest('[hidden], [aria-hidden="true"], [inert]') &&
      getComputedStyle(element).visibility !== 'hidden'
    const width = enabled ? Math.max(element.clientWidth, root.offsetWidth) : 0
    const height = enabled ? Math.max(element.clientHeight, root.offsetHeight) : 0
    const scaleX = enabled ? origin.width / element.offsetWidth : 1
    const scaleY = enabled ? origin.height / element.offsetHeight : 1
    const nodes: TreeNodeBounds[] = enabled
      ? [...root.querySelectorAll<HTMLElement>('.agent-tree__node')].map((node) => {
          const rect = node.getBoundingClientRect()
          const id = node.dataset.agentId ?? null
          return {
            id,
            parentId: id ? (parentById.get(id) ?? null) : null,
            left: (rect.left - origin!.left) / scaleX + element.scrollLeft,
            right: (rect.right - origin!.left) / scaleX + element.scrollLeft,
            top: (rect.top - origin!.top) / scaleY + element.scrollTop,
            bottom: (rect.bottom - origin!.top) / scaleY + element.scrollTop
          }
        })
      : []
    const now = Date.now()
    for (const event of transmissions) {
      if (seen.current.has(event.id)) continue
      seen.current.add(event.id)
      // Hidden views/endpoints consume an event without replaying it when reopened later.
      if (!enabled || now - event.receivedAt > MAX_AGE_MS || event.receivedAt > now + 1000) continue
      if (
        !nodes.some((node) => node.id === event.sourceAgentId) ||
        !nodes.some((node) => node.id === event.targetAgentId)
      )
        continue
      if (queued.current.length < MAX_QUEUED) queued.current.push(event)
    }
    // Store events are short-lived; a bounded seen set also protects long-lived open trees.
    while (seen.current.size > 1024) seen.current.delete(seen.current.values().next().value!)
    if (!enabled) {
      queued.current = []
      active.current = []
    } else {
      active.current = active.current.flatMap((event) => {
        if (now - event.startedAt > 1600) return []
        const route = routeAgentTreeTransmission(event, nodes, width, height)
        return route ? [{ ...event, route }] : []
      })
      queued.current = queued.current.filter((event) => now - event.receivedAt <= MAX_AGE_MS)
      while (active.current.length < MAX_ACTIVE && queued.current.length) {
        const available = queued.current.findIndex(
          (event) =>
            !active.current.some(
              (playing) =>
                (playing.sourceAgentId === event.sourceAgentId &&
                  playing.targetAgentId === event.targetAgentId) ||
                (playing.sourceAgentId === event.targetAgentId &&
                  playing.targetAgentId === event.sourceAgentId)
            )
        )
        if (available < 0) break
        const [event] = queued.current.splice(available, 1)
        const route = routeAgentTreeTransmission(event, nodes, width, height)
        if (route) active.current.push({ ...event, route, startedAt: now })
      }
    }
    const next = { width, height, active: active.current }
    setView((current) =>
      current.width === width &&
      current.height === height &&
      current.active.length === next.active.length &&
      current.active.every(
        (event, index) =>
          event.id === next.active[index].id && event.route.path === next.active[index].route.path
      )
        ? current
        : next
    )
  }, [])

  useLayoutEffect(() => {
    environment.current = { layout, transmissions, agents, scrollRef }
    refresh()
  }, [layout, transmissions, agents, collapsedAgentIds, userCollapsed, scrollRef, refresh])

  useLayoutEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)')
    const updateMotion = () => {
      reducedMotion.current = media.matches
      refresh()
    }
    updateMotion()
    media.addEventListener('change', updateMotion)
    const observer = new ResizeObserver(refresh)
    const visibilityObserver = new MutationObserver(refresh)
    const element = scrollRef.current
    if (element) {
      observer.observe(element)
      const root = element.querySelector('.agent-tree__roots')
      if (root) observer.observe(root)
      let ancestor: HTMLElement | null = element
      while (ancestor) {
        visibilityObserver.observe(ancestor, {
          attributes: true,
          attributeFilter: ['style', 'class', 'hidden', 'aria-hidden', 'inert']
        })
        ancestor = ancestor.parentElement
      }
    }
    document.addEventListener('visibilitychange', refresh)
    return () => {
      observer.disconnect()
      visibilityObserver.disconnect()
      media.removeEventListener('change', updateMotion)
      document.removeEventListener('visibilitychange', refresh)
    }
  }, [scrollRef, layout, refresh])

  useLayoutEffect(() => {
    if (!view.active.length) return undefined
    // animationend is primary; this deadline also settles suspended/removed CSS animations.
    const deadline = Math.min(...view.active.map((event) => event.startedAt + 1601))
    const timer = window.setTimeout(refresh, Math.max(0, deadline - Date.now()))
    return () => window.clearTimeout(timer)
  }, [view.active, refresh])

  const finish = useCallback(
    (id: string) => {
      active.current = active.current.filter((event) => event.id !== id)
      refresh()
    },
    [refresh]
  )

  return { ...view, finish }
}
