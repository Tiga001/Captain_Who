// Renderer startup tests: verify that the pre-React branded mask streams ambient text.

import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  startBootstrapStartupAmbientText,
  stopBootstrapStartupAmbientText
} from '../../features/startup/bootstrapStartupAmbientText'
import {
  STARTUP_PHRASE_FADE_MS,
  STARTUP_PHRASE_HOLD_MS
} from '../../features/startup/startupAmbientPhrases'

afterEach(() => {
  stopBootstrapStartupAmbientText()
  vi.restoreAllMocks()
  vi.useRealTimers()
  document.querySelector('[data-bootstrap-startup-ambient]')?.remove()
})

describe('bootstrap startup ambient text', () => {
  it('animates the existing pre-React mask instead of waiting for the application mount', async () => {
    vi.useFakeTimers()
    vi.spyOn(Math, 'random').mockReturnValue(0)
    vi.spyOn(window, 'matchMedia').mockReturnValue({
      matches: false,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn()
    } as unknown as MediaQueryList)
    const ambient = document.createElement('div')
    ambient.dataset.bootstrapStartupAmbient = ''
    document.body.append(ambient)

    startBootstrapStartupAmbientText(document, 'zh-CN')

    expect(ambient.dataset.phase).toBe('holding')
    expect(ambient.textContent?.length).toBeGreaterThan(0)
    const bootstrapPhrase = ambient.textContent

    await vi.advanceTimersByTimeAsync(STARTUP_PHRASE_HOLD_MS)
    expect(ambient.dataset.phase).toBe('fading')

    await vi.advanceTimersByTimeAsync(STARTUP_PHRASE_FADE_MS)
    expect(ambient.dataset.phase).toBe('typing')
    expect(ambient.textContent).toBe('')

    await vi.advanceTimersToNextTimerAsync()
    expect(ambient.dataset.phase).toBe('typing')
    expect(ambient.textContent?.length).toBe(1)
    expect(ambient.textContent).not.toBe(bootstrapPhrase)
  })
})
