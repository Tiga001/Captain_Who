import { useCallback, useEffect, useRef, useState } from 'react'
import {
  CENTER_MIN_WIDTH,
  LEFT_DEFAULT_WIDTH,
  LEFT_MIN_WIDTH,
  RIGHT_DEFAULT_WIDTH,
  RIGHT_MIN_WIDTH,
  SIDE_MAX_WIDTH,
  clamp
} from './appConstants'
import type { Side } from './appTypes'

export function useShellLayout() {
  const shellRef = useRef<HTMLDivElement>(null)
  const [leftWidth, setLeftWidth] = useState(LEFT_DEFAULT_WIDTH)
  const [rightWidth, setRightWidth] = useState(RIGHT_DEFAULT_WIDTH)
  const [leftOpen, setLeftOpen] = useState(true)
  const [rightOpen, setRightOpen] = useState(false)
  const [rightMaximized, setRightMaximized] = useState(false)

  const resizeSide = useCallback(
    (side: Side, deltaX: number) => {
      const shellWidth = shellRef.current?.clientWidth ?? window.innerWidth
      const otherWidth = side === 'left' ? (rightOpen ? rightWidth : 0) : leftOpen ? leftWidth : 0
      const minimum = side === 'left' ? LEFT_MIN_WIDTH : RIGHT_MIN_WIDTH
      const availableMax = Math.max(minimum, shellWidth - otherWidth - CENTER_MIN_WIDTH)
      const maximum = Math.min(SIDE_MAX_WIDTH, availableMax)

      if (side === 'left') {
        setLeftWidth((currentWidth) => clamp(currentWidth + deltaX, minimum, maximum))
      } else {
        setRightWidth((currentWidth) => clamp(currentWidth - deltaX, minimum, maximum))
      }
    },
    [leftOpen, leftWidth, rightOpen, rightWidth]
  )

  const getConstrainedLayout = useCallback(
    (requestedLeftOpen: boolean, requestedRightOpen: boolean, preferredSide?: Side) => {
      const shellWidth = shellRef.current?.clientWidth ?? window.innerWidth
      let nextLeftOpen = requestedLeftOpen
      let nextRightOpen = requestedRightOpen
      let nextLeftWidth = clamp(leftWidth, LEFT_MIN_WIDTH, SIDE_MAX_WIDTH)
      let nextRightWidth = clamp(rightWidth, RIGHT_MIN_WIDTH, SIDE_MAX_WIDTH)

      const getMinimumOpenWidth = () =>
        CENTER_MIN_WIDTH +
        (nextLeftOpen ? LEFT_MIN_WIDTH : 0) +
        (nextRightOpen ? RIGHT_MIN_WIDTH : 0)

      if (shellWidth < getMinimumOpenWidth()) {
        if (preferredSide === 'left') {
          nextRightOpen = false
          if (shellWidth < getMinimumOpenWidth()) {
            nextLeftOpen = false
          }
        } else if (preferredSide === 'right') {
          nextLeftOpen = false
          if (shellWidth < getMinimumOpenWidth()) {
            nextRightOpen = false
          }
        } else {
          nextRightOpen = false
          if (shellWidth < getMinimumOpenWidth()) {
            nextLeftOpen = false
          }
        }
      }

      const maxOpenSideWidth = Math.max(0, shellWidth - CENTER_MIN_WIDTH)
      let overflow =
        (nextLeftOpen ? nextLeftWidth : 0) + (nextRightOpen ? nextRightWidth : 0) - maxOpenSideWidth

      const shrinkLeft = () => {
        if (!nextLeftOpen || overflow <= 0) return
        const shrinkAmount = Math.min(overflow, Math.max(0, nextLeftWidth - LEFT_MIN_WIDTH))
        nextLeftWidth -= shrinkAmount
        overflow -= shrinkAmount
      }

      const shrinkRight = () => {
        if (!nextRightOpen || overflow <= 0) return
        const shrinkAmount = Math.min(overflow, Math.max(0, nextRightWidth - RIGHT_MIN_WIDTH))
        nextRightWidth -= shrinkAmount
        overflow -= shrinkAmount
      }

      if (overflow > 0) {
        const leftExcess = nextLeftOpen ? nextLeftWidth - LEFT_MIN_WIDTH : 0
        const rightExcess = nextRightOpen ? nextRightWidth - RIGHT_MIN_WIDTH : 0

        if (preferredSide === 'left') {
          shrinkRight()
          shrinkLeft()
        } else if (preferredSide === 'right') {
          shrinkLeft()
          shrinkRight()
        } else if (leftExcess >= rightExcess) {
          shrinkLeft()
          shrinkRight()
        } else {
          shrinkRight()
          shrinkLeft()
        }
      }

      return {
        leftOpen: nextLeftOpen,
        leftWidth: nextLeftWidth,
        rightOpen: nextRightOpen,
        rightWidth: nextRightWidth
      }
    },
    [leftWidth, rightWidth]
  )

  const applyConstrainedLayout = useCallback(
    (requestedLeftOpen: boolean, requestedRightOpen: boolean, preferredSide?: Side) => {
      const nextLayout = getConstrainedLayout(requestedLeftOpen, requestedRightOpen, preferredSide)

      if (nextLayout.leftOpen !== leftOpen) {
        setLeftOpen(nextLayout.leftOpen)
      }
      if (nextLayout.rightOpen !== rightOpen) {
        setRightOpen(nextLayout.rightOpen)
      }
      if (Math.abs(nextLayout.leftWidth - leftWidth) > 0.5) {
        setLeftWidth(nextLayout.leftWidth)
      }
      if (Math.abs(nextLayout.rightWidth - rightWidth) > 0.5) {
        setRightWidth(nextLayout.rightWidth)
      }
    },
    [getConstrainedLayout, leftOpen, leftWidth, rightOpen, rightWidth]
  )

  const toggleLeftSidebar = useCallback(() => {
    const nextLeftOpen = !leftOpen
    applyConstrainedLayout(nextLeftOpen, rightOpen, nextLeftOpen ? 'left' : undefined)
  }, [applyConstrainedLayout, leftOpen, rightOpen])

  const toggleRightSidebar = useCallback(() => {
    const nextRightOpen = !rightOpen
    applyConstrainedLayout(leftOpen, nextRightOpen, nextRightOpen ? 'right' : undefined)
  }, [applyConstrainedLayout, leftOpen, rightOpen])

  const toggleRightSidebarMaximized = useCallback(() => {
    setRightMaximized((isMaximized) => !isMaximized)
  }, [])

  useEffect(() => {
    const keepCenterVisible = () => {
      applyConstrainedLayout(leftOpen, rightOpen)
    }

    keepCenterVisible()
    window.addEventListener('resize', keepCenterVisible)
    return () => window.removeEventListener('resize', keepCenterVisible)
  }, [applyConstrainedLayout, leftOpen, rightOpen])

  return {
    leftOpen,
    leftWidth,
    resizeSide,
    rightMaximized,
    rightOpen,
    rightWidth,
    shellRef,
    toggleLeftSidebar,
    toggleRightSidebar,
    toggleRightSidebarMaximized
  }
}
