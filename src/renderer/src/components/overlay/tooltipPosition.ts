export type TooltipPlacement = 'top' | 'bottom'

export interface TooltipPositionInput {
  anchorRect: Pick<DOMRect, 'bottom' | 'height' | 'left' | 'right' | 'top' | 'width'>
  gap: number
  padding: number
  preferredPlacement: TooltipPlacement
  tooltipHeight: number
  tooltipWidth: number
  viewportHeight: number
  viewportWidth: number
}

export interface TooltipPosition {
  left: number
  placement: TooltipPlacement
  top: number
}

/**
 * Positions a fixed overlay without depending on any scroll container. The preferred side wins
 * until it clips and the opposite side has more usable space; the final rectangle is always
 * clamped to the viewport safety margin.
 */
export function computeTooltipPosition({
  anchorRect,
  gap,
  padding,
  preferredPlacement,
  tooltipHeight,
  tooltipWidth,
  viewportHeight,
  viewportWidth
}: TooltipPositionInput): TooltipPosition {
  const availableAbove = anchorRect.top - padding
  const availableBelow = viewportHeight - anchorRect.bottom - padding
  const requiredHeight = tooltipHeight + gap
  const oppositePlacement: TooltipPlacement = preferredPlacement === 'top' ? 'bottom' : 'top'
  const preferredSpace = preferredPlacement === 'top' ? availableAbove : availableBelow
  const oppositeSpace = preferredPlacement === 'top' ? availableBelow : availableAbove
  const placement =
    preferredSpace >= requiredHeight || preferredSpace >= oppositeSpace
      ? preferredPlacement
      : oppositePlacement

  const unclampedLeft = anchorRect.left + anchorRect.width / 2 - tooltipWidth / 2
  const maximumLeft = Math.max(padding, viewportWidth - tooltipWidth - padding)
  const left = clamp(unclampedLeft, padding, maximumLeft)
  const unclampedTop =
    placement === 'top' ? anchorRect.top - tooltipHeight - gap : anchorRect.bottom + gap
  const maximumTop = Math.max(padding, viewportHeight - tooltipHeight - padding)
  const top = clamp(unclampedTop, padding, maximumTop)

  return { left, placement, top }
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum)
}
