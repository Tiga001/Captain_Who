import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  LEFT_DEFAULT_WIDTH,
  LEFT_MIN_WIDTH,
  RIGHT_DEFAULT_WIDTH,
  RIGHT_MIN_WIDTH,
  clamp
} from './appConstants'
import { getSidebarResizeMaximum, resolveShellLayout } from './shellLayoutPolicy'
import type { SidebarResizeMetrics, SidebarSide } from '../lib/sidebarResize'

function readShellWidth(element: HTMLDivElement | null): number {
  return element?.clientWidth || window.innerWidth
}

export function useShellLayout() {
  const shellRef = useRef<HTMLDivElement>(null)
  const [shellWidth, setShellWidth] = useState(() => window.innerWidth)
  const [leftPreferredWidth, setLeftPreferredWidth] = useState(LEFT_DEFAULT_WIDTH)
  const [rightPreferredWidth, setRightPreferredWidth] = useState(RIGHT_DEFAULT_WIDTH)
  const [leftRequestedOpen, setLeftRequestedOpen] = useState(true)
  const [rightRequestedOpen, setRightRequestedOpen] = useState(false)
  const [preferredSide, setPreferredSide] = useState<SidebarSide | undefined>()
  const [rightMaximized, setRightMaximized] = useState(false)

  const layout = useMemo(
    () =>
      resolveShellLayout({
        shellWidth,
        leftRequestedOpen,
        rightRequestedOpen,
        leftPreferredWidth,
        rightPreferredWidth,
        preferredSide
      }),
    [
      leftPreferredWidth,
      leftRequestedOpen,
      preferredSide,
      rightPreferredWidth,
      rightRequestedOpen,
      shellWidth
    ]
  )

  const leftResizeMetrics = useMemo<SidebarResizeMetrics>(
    () => ({
      maximum: getSidebarResizeMaximum(
        'left',
        shellWidth,
        layout.rightOpen ? layout.rightWidth : 0
      ),
      minimum: LEFT_MIN_WIDTH,
      width: layout.leftWidth
    }),
    [layout.rightOpen, layout.rightWidth, layout.leftWidth, shellWidth]
  )

  const rightResizeMetrics = useMemo<SidebarResizeMetrics>(
    () => ({
      maximum: getSidebarResizeMaximum('right', shellWidth, layout.leftOpen ? layout.leftWidth : 0),
      minimum: RIGHT_MIN_WIDTH,
      width: layout.rightWidth
    }),
    [layout.leftOpen, layout.leftWidth, layout.rightWidth, shellWidth]
  )

  const commitSidebarResize = useCallback(
    (side: SidebarSide, width: number) => {
      setPreferredSide(side)

      if (side === 'left') {
        setLeftPreferredWidth(clamp(width, leftResizeMetrics.minimum, leftResizeMetrics.maximum))
        return
      }

      setRightPreferredWidth(clamp(width, rightResizeMetrics.minimum, rightResizeMetrics.maximum))
    },
    [leftResizeMetrics, rightResizeMetrics]
  )

  const toggleLeftSidebar = useCallback(() => {
    if (layout.leftOpen) {
      setLeftRequestedOpen(false)
      setPreferredSide(rightRequestedOpen ? 'right' : undefined)
      return
    }

    setLeftRequestedOpen(true)
    setPreferredSide('left')
  }, [layout.leftOpen, rightRequestedOpen])

  const toggleRightSidebar = useCallback(() => {
    if (layout.rightOpen) {
      setRightRequestedOpen(false)
      setRightMaximized(false)
      setPreferredSide(leftRequestedOpen ? 'left' : undefined)
      return
    }

    setRightRequestedOpen(true)
    setPreferredSide('right')
  }, [layout.rightOpen, leftRequestedOpen])

  const openRightSidebar = useCallback(() => {
    setRightRequestedOpen(true)
    setPreferredSide('right')
  }, [])

  const toggleRightSidebarMaximized = useCallback(() => {
    setRightMaximized((isMaximized) => !isMaximized)
  }, [])

  useEffect(() => {
    const shell = shellRef.current
    const updateShellWidth = () => {
      const nextWidth = readShellWidth(shell)
      setShellWidth((currentWidth) =>
        Math.abs(currentWidth - nextWidth) > 0.5 ? nextWidth : currentWidth
      )
    }

    updateShellWidth()
    window.addEventListener('resize', updateShellWidth)

    const resizeObserver = shell ? new ResizeObserver(updateShellWidth) : null
    if (shell) resizeObserver?.observe(shell)

    return () => {
      resizeObserver?.disconnect()
      window.removeEventListener('resize', updateShellWidth)
    }
  }, [])

  return {
    commitSidebarResize,
    leftResizeMetrics,
    leftOpen: layout.leftOpen,
    leftWidth: layout.leftWidth,
    openRightSidebar,
    rightMaximized,
    rightOpen: layout.rightOpen,
    rightResizeMetrics,
    rightWidth: layout.rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  }
}
