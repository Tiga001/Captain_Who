import { describe, expect, it } from 'vitest'
import {
  computeTooltipPosition,
  type TooltipPositionInput
} from '../../../components/overlay/tooltipPosition'

function createInput(overrides: Partial<TooltipPositionInput> = {}): TooltipPositionInput {
  return {
    anchorRect: {
      bottom: 120,
      height: 20,
      left: 100,
      right: 140,
      top: 100,
      width: 40
    },
    gap: 8,
    padding: 8,
    preferredPlacement: 'top',
    tooltipHeight: 24,
    tooltipWidth: 80,
    viewportHeight: 300,
    viewportWidth: 300,
    ...overrides
  }
}

describe('computeTooltipPosition', () => {
  it('keeps the preferred placement when it has enough room', () => {
    expect(computeTooltipPosition(createInput())).toEqual({
      left: 80,
      placement: 'top',
      top: 68
    })
  })

  it('flips below the anchor when the preferred top side clips and the bottom has more room', () => {
    const position = computeTooltipPosition(
      createInput({
        anchorRect: {
          bottom: 30,
          height: 20,
          left: 100,
          right: 140,
          top: 10,
          width: 40
        },
        tooltipHeight: 40
      })
    )

    expect(position).toEqual({ left: 80, placement: 'bottom', top: 38 })
  })

  it('keeps the preferred side when neither side fits but it has more usable space', () => {
    const position = computeTooltipPosition(
      createInput({
        anchorRect: {
          bottom: 60,
          height: 10,
          left: 100,
          right: 140,
          top: 50,
          width: 40
        },
        tooltipHeight: 60,
        viewportHeight: 80
      })
    )

    expect(position).toEqual({ left: 80, placement: 'top', top: 8 })
  })

  it.each([
    [0, 16, 8],
    [190, 10, 92]
  ])('clamps a tooltip anchored at x=%i to the viewport padding', (left, width, expectedLeft) => {
    const position = computeTooltipPosition(
      createInput({
        anchorRect: {
          bottom: 120,
          height: 20,
          left,
          right: left + width,
          top: 100,
          width
        },
        tooltipWidth: 100,
        viewportWidth: 200
      })
    )

    expect(position.left).toBe(expectedLeft)
  })
})
