import { describe, expect, it } from 'vitest'
import { formatCacheHitRate } from '../../features/agent/usagePresentation'

describe('cache hit rate presentation', () => {
  it.each([
    [9011, 10000, '90.11%'],
    [1, 6, '16.67%'],
    [0, 100, '0.00%'],
    [100, 100, '100.00%'],
    [9, 10, '90.00%']
  ])('formats %s cache hits out of %s input tokens as %s', (cached, input, expected) => {
    expect(formatCacheHitRate(cached, input)).toBe(expected)
  })

  it.each([
    [undefined, 100],
    [null, 100],
    [10, undefined],
    [0, 0],
    [1, 0],
    [-1, 100],
    [1, -100],
    [101, 100],
    [NaN, 100],
    [1, Infinity]
  ])('does not invent a rate for invalid usage (%s, %s)', (cached, input) => {
    expect(formatCacheHitRate(cached, input)).toBeNull()
  })
})
