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
import {
  BOTTOM_PANEL_DEFAULT_HEIGHT,
  BOTTOM_PANEL_MIN_HEIGHT,
  resolveBottomPanelLayout,
  type BottomPanelResizeMetrics
} from './bottomPanelLayout'

function readShellWidth(element: HTMLDivElement | null): number {
  return element?.clientWidth || window.innerWidth
}

export function useShellLayout() {
  const shellRef = useRef<HTMLDivElement>(null)
  const [shellWidth, setShellWidth] = useState(() => window.innerWidth)
  const [shellHeight, setShellHeight] = useState(() => window.innerHeight)
  const [leftPreferredWidth, setLeftPreferredWidth] = useState(LEFT_DEFAULT_WIDTH)
  const [rightPreferredWidth, setRightPreferredWidth] = useState(RIGHT_DEFAULT_WIDTH)
  const [leftRequestedOpen, setLeftRequestedOpen] = useState(true)
  const [rightRequestedOpen, setRightRequestedOpen] = useState(false)
  const [preferredSide, setPreferredSide] = useState<SidebarSide | undefined>()
  const [rightMaximized, setRightMaximized] = useState(false)
  const [bottomRequestedOpen, setBottomRequestedOpen] = useState(false)
  const [bottomPreferredHeight, setBottomPreferredHeight] = useState(BOTTOM_PANEL_DEFAULT_HEIGHT)
  const bottomLayout = resolveBottomPanelLayout(
    shellHeight,
    bottomRequestedOpen,
    bottomPreferredHeight
  )
  const bottomResizeMetrics: BottomPanelResizeMetrics = {
    height: bottomLayout.height,
    maximum: bottomLayout.maximum,
    minimum: BOTTOM_PANEL_MIN_HEIGHT
  }
  const closeBottomPanel = useCallback(() => setBottomRequestedOpen(false), [])
  const toggleBottomPanel = useCallback(
    () => setBottomRequestedOpen(!bottomLayout.open),
    [bottomLayout.open]
  )
  const commitBottomPanelResize = useCallback(
    (_side: 'bottom', height: number) => {
      setBottomPreferredHeight(clamp(height, BOTTOM_PANEL_MIN_HEIGHT, bottomLayout.maximum))
    },
    [bottomLayout.maximum]
  )

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
      const nextHeight = shell?.clientHeight || window.innerHeight
      setShellWidth((currentWidth) =>
        Math.abs(currentWidth - nextWidth) > 0.5 ? nextWidth : currentWidth
      )
      setShellHeight((currentHeight) =>
        Math.abs(currentHeight - nextHeight) > 0.5 ? nextHeight : currentHeight
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
    bottomHeight: bottomLayout.height,
    bottomOpen: bottomLayout.open,
    bottomResizeMetrics,
    closeBottomPanel,
    commitBottomPanelResize,
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
    toggleBottomPanel,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  }
}
