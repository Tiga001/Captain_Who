// Timeline elapsed label: second precision is kept after an hour; zero units are omitted.
import { describe, expect, it, vi } from 'vitest'
import { formatElapsedDuration } from '../components/chatMessageItemUtils'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

describe('formatElapsedDuration', () => {
  it('omits zero units below one hour', () => {
    expect(formatElapsedDuration(0)).toBe('0s')
    expect(formatElapsedDuration(-1_000)).toBe('0s')
    expect(formatElapsedDuration(45_000)).toBe('45s')
    expect(formatElapsedDuration(60_000)).toBe('1m')
    expect(formatElapsedDuration(90_000)).toBe('1m 30s')
    expect(formatElapsedDuration(3_599_000)).toBe('59m 59s')
  })

  it('keeps second precision once the duration reaches an hour', () => {
    expect(formatElapsedDuration(3_600_000)).toBe('1h')
    expect(formatElapsedDuration(3_600_000 + 5_000)).toBe('1h 5s')
    expect(formatElapsedDuration(3_600_000 + 60_000)).toBe('1h 1m')
    expect(formatElapsedDuration(3_600_000 + 60_000 + 5_000)).toBe('1h 1m 5s')
    expect(formatElapsedDuration(9_045_000)).toBe('2h 30m 45s')
  })

  it('applies the same zero-omission rule to days', () => {
    expect(formatElapsedDuration(24 * 3_600_000)).toBe('1d')
    expect(formatElapsedDuration(25 * 3_600_000)).toBe('1d 1h')
    expect(formatElapsedDuration(24 * 3_600_000 + 30 * 60_000)).toBe('1d 30m')
    expect(formatElapsedDuration(25 * 3_600_000 + 65_000)).toBe('1d 1h 1m 5s')
  })
})
