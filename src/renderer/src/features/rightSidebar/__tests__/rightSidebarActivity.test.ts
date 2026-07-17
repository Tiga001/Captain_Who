import { describe, expect, it } from 'vitest'
import { resolveRightSidebarActivity } from '../rightSidebarActivity'

describe('right sidebar activity', () => {
  it.each([
    {
      documentVisible: true,
      expected: 'foreground',
      isSelected: true,
      sidebarVisible: true
    },
    {
      documentVisible: true,
      expected: 'background',
      isSelected: false,
      sidebarVisible: true
    },
    {
      documentVisible: true,
      expected: 'dormant',
      isSelected: true,
      sidebarVisible: false
    },
    {
      documentVisible: false,
      expected: 'dormant',
      isSelected: true,
      sidebarVisible: true
    },
    {
      documentVisible: false,
      expected: 'dormant',
      isSelected: false,
      sidebarVisible: false
    }
  ] as const)(
    'resolves selected=$isSelected sidebar=$sidebarVisible document=$documentVisible as $expected',
    ({ expected, ...input }) => {
      expect(resolveRightSidebarActivity(input)).toBe(expected)
    }
  )
})
