import {
  CENTER_MIN_WIDTH,
  LEFT_MAX_WIDTH,
  LEFT_MIN_WIDTH,
  RIGHT_MAX_VIEWPORT_RATIO,
  RIGHT_MAX_WIDTH,
  RIGHT_MIN_WIDTH,
  clamp
} from './appConstants'
import type { Side } from './appTypes'

export interface ShellLayoutIntent {
  shellWidth: number
  leftRequestedOpen: boolean
  rightRequestedOpen: boolean
  leftPreferredWidth: number
  rightPreferredWidth: number
  preferredSide?: Side
}

export interface ResolvedShellLayout {
  leftOpen: boolean
  leftWidth: number
  centerWidth: number
  rightOpen: boolean
  rightWidth: number
}

export function getRightMaximumWidth(shellWidth: number): number {
  return Math.max(
    RIGHT_MIN_WIDTH,
    Math.min(RIGHT_MAX_WIDTH, Math.max(0, shellWidth) * RIGHT_MAX_VIEWPORT_RATIO)
  )
}

export function getSidebarResizeMaximum(
  side: Side,
  shellWidth: number,
  otherVisibleWidth: number
): number {
  const minimum = side === 'left' ? LEFT_MIN_WIDTH : RIGHT_MIN_WIDTH
  const configuredMaximum = side === 'left' ? LEFT_MAX_WIDTH : getRightMaximumWidth(shellWidth)
  const availableMaximum = Math.max(
    minimum,
    Math.max(0, shellWidth) - Math.max(0, otherVisibleWidth) - CENTER_MIN_WIDTH
  )

  return Math.min(configuredMaximum, availableMaximum)
}

/**
 * Resolves the user's sidebar intent against the current shell width. The caller keeps the
 * preferred widths and requested visibility unchanged, so temporary constraints can be reversed.
 */
export function resolveShellLayout(intent: ShellLayoutIntent): ResolvedShellLayout {
  const shellWidth = Math.max(0, intent.shellWidth)
  let leftOpen = intent.leftRequestedOpen
  let rightOpen = intent.rightRequestedOpen
  let leftWidth = clamp(intent.leftPreferredWidth, LEFT_MIN_WIDTH, LEFT_MAX_WIDTH)
  let rightWidth = clamp(
    intent.rightPreferredWidth,
    RIGHT_MIN_WIDTH,
    getRightMaximumWidth(shellWidth)
  )

  const minimumRequiredWidth = () =>
    CENTER_MIN_WIDTH + (leftOpen ? LEFT_MIN_WIDTH : 0) + (rightOpen ? RIGHT_MIN_WIDTH : 0)

  if (shellWidth < minimumRequiredWidth()) {
    if (intent.preferredSide === 'left') {
      rightOpen = false
      if (shellWidth < minimumRequiredWidth()) leftOpen = false
    } else if (intent.preferredSide === 'right') {
      leftOpen = false
      if (shellWidth < minimumRequiredWidth()) rightOpen = false
    } else {
      rightOpen = false
      if (shellWidth < minimumRequiredWidth()) leftOpen = false
    }
  }

  let overflow =
    (leftOpen ? leftWidth : 0) +
    (rightOpen ? rightWidth : 0) -
    Math.max(0, shellWidth - CENTER_MIN_WIDTH)

  const shrinkLeft = () => {
    if (!leftOpen || overflow <= 0) return
    const amount = Math.min(overflow, Math.max(0, leftWidth - LEFT_MIN_WIDTH))
    leftWidth -= amount
    overflow -= amount
  }

  const shrinkRight = () => {
    if (!rightOpen || overflow <= 0) return
    const amount = Math.min(overflow, Math.max(0, rightWidth - RIGHT_MIN_WIDTH))
    rightWidth -= amount
    overflow -= amount
  }

  if (intent.preferredSide === 'right') {
    shrinkLeft()
    shrinkRight()
  } else {
    // Natural window resizing and explicit left-sidebar actions preserve navigation first.
    shrinkRight()
    shrinkLeft()
  }

  return {
    leftOpen,
    leftWidth,
    centerWidth: Math.max(
      0,
      shellWidth - (leftOpen ? leftWidth : 0) - (rightOpen ? rightWidth : 0)
    ),
    rightOpen,
    rightWidth
  }
}
