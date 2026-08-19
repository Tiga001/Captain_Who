import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  LEFT_DEFAULT_WIDTH,
  LEFT_MIN_WIDTH,
  RIGHT_DEFAULT_WIDTH,
  RIGHT_MIN_WIDTH,
  clamp
} from './appConstants'
import { getSidebarResizeMaximum, resolveShellLayout } from './shellLayoutPolicy'
import type { Side } from './appTypes'

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
  const [preferredSide, setPreferredSide] = useState<Side | undefined>()
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

  const resizeSide = useCallback(
    (side: Side, deltaX: number) => {
      setPreferredSide(side)

      if (side === 'left') {
        const maximum = getSidebarResizeMaximum(
          'left',
          shellWidth,
          layout.rightOpen ? layout.rightWidth : 0
        )
        setLeftPreferredWidth(clamp(layout.leftWidth + deltaX, LEFT_MIN_WIDTH, maximum))
        return
      }

      const maximum = getSidebarResizeMaximum(
        'right',
        shellWidth,
        layout.leftOpen ? layout.leftWidth : 0
      )
      setRightPreferredWidth(clamp(layout.rightWidth - deltaX, RIGHT_MIN_WIDTH, maximum))
    },
    [layout.leftOpen, layout.leftWidth, layout.rightOpen, layout.rightWidth, shellWidth]
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
    leftOpen: layout.leftOpen,
    leftWidth: layout.leftWidth,
    openRightSidebar,
    resizeSide,
    rightMaximized,
    rightOpen: layout.rightOpen,
    rightWidth: layout.rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  }
}
