import { useCallback, useLayoutEffect, useState } from 'react'
import type { BasicToolItem } from './basicToolTimeline'

type Group = { items: BasicToolItem[] }

// Expansion belongs to operation anchors, not presentation containers: a late narration or
// collaboration boundary can split a span without closing its existing details.
export function useBasicToolExpansion(runId: string | null, groups: readonly Group[]) {
  const [state, setState] = useState<{
    runId: string | null
    byItem: Record<string, boolean>
    details: Record<string, boolean>
  }>({ runId, byItem: {}, details: {} })
  const byItem = state.runId === runId ? state.byItem : {}
  const isExpanded = (items: readonly BasicToolItem[]) =>
    items.some((item) => byItem[item.id] === true)

  useLayoutEffect(() => {
    setState((current) => {
      const previous = current.runId === runId ? current.byItem : {}
      const details = current.runId === runId ? current.details : {}
      let next = previous
      for (const group of groups) {
        const singleton = group.items.length === 1 ? group.items[0] : undefined
        const expanded =
          singleton && details[singleton.id] !== undefined
            ? details[singleton.id]
            : group.items.some((item) => previous[item.id] === true)
        for (const item of group.items) {
          if (next[item.id] === expanded) continue
          if (next === previous) next = { ...previous }
          next[item.id] = expanded
        }
      }
      if (current.runId === runId && next === previous) return current
      return { runId, byItem: next, details }
    })
  }, [groups, runId, state.details])

  const setExpanded = useCallback(
    (items: readonly BasicToolItem[], expanded: boolean) => {
      setState((current) => {
        const previous = current.runId === runId ? current.byItem : {}
        if (items.every((item) => previous[item.id] === expanded)) return current
        const next = { ...previous }
        for (const item of items) next[item.id] = expanded
        return {
          runId,
          byItem: next,
          details: current.runId === runId ? current.details : {}
        }
      })
    },
    [runId]
  )

  const setDetailExpanded = useCallback(
    (item: BasicToolItem, expanded: boolean) => {
      setState((current) => {
        const details = current.runId === runId ? current.details : {}
        if (details[item.id] === expanded) return current
        return {
          runId,
          byItem: current.runId === runId ? current.byItem : {},
          details: { ...details, [item.id]: expanded }
        }
      })
    },
    [runId]
  )

  return { isExpanded, setExpanded, setDetailExpanded }
}
