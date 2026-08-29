// Renderer startup layer: animate the existing pre-React launch surface while Host initializes.

import { getTranslation } from '../../config/languageRegistry'
import type { AppLanguage, TranslationKey } from '../../config/languageRegistry'
import {
  getStartupCharacterIntervalMs,
  pickFirstStartupPhraseIndex,
  pickNextStartupPhraseIndex,
  pickStartupPhraseGroupIndex,
  STARTUP_AMBIENT_PHRASE_GROUPS,
  STARTUP_BOOTSTRAP_PHRASE_KEY,
  STARTUP_PHRASE_FADE_MS,
  STARTUP_PHRASE_HOLD_MS,
  STARTUP_REDUCED_MOTION_HOLD_MS
} from './startupAmbientPhrases'

let stopActiveAnimation: (() => void) | null = null

export function stopBootstrapStartupAmbientText(): void {
  stopActiveAnimation?.()
  stopActiveAnimation = null
}

export function startBootstrapStartupAmbientText(
  documentRoot: Document,
  language: AppLanguage
): void {
  stopBootstrapStartupAmbientText()
  const ambientText = documentRoot.querySelector<HTMLElement>('[data-bootstrap-startup-ambient]')
  if (!ambientText) return

  const reducedMotionQuery = window.matchMedia?.('(prefers-reduced-motion: reduce)')
  let prefersReducedMotion = reducedMotionQuery?.matches ?? false
  const phraseGroup =
    STARTUP_AMBIENT_PHRASE_GROUPS[pickStartupPhraseGroupIndex(STARTUP_AMBIENT_PHRASE_GROUPS.length)]
  let phraseKey: TranslationKey = STARTUP_BOOTSTRAP_PHRASE_KEY
  let phraseIndex: number | null = null
  let phase: 'typing' | 'holding' | 'fading' = 'holding'
  let visibleCharacterCount = Array.from(getTranslation(language, phraseKey)).length
  let timeoutId: number | null = null
  let stopped = false

  const render = (): void => {
    const characters = Array.from(getTranslation(language, phraseKey))
    ambientText.dataset.phase = phase
    ambientText.textContent = prefersReducedMotion
      ? characters.join('')
      : characters.slice(0, visibleCharacterCount).join('')
  }

  const schedule = (callback: () => void, delayMs: number): void => {
    timeoutId = window.setTimeout(callback, delayMs)
  }

  const selectNextPhrase = (): void => {
    phraseIndex =
      phraseIndex === null
        ? pickFirstStartupPhraseIndex(phraseGroup, STARTUP_BOOTSTRAP_PHRASE_KEY)
        : pickNextStartupPhraseIndex(phraseIndex, phraseGroup.length)
    phraseKey = phraseGroup[phraseIndex]
  }

  const advance = (): void => {
    if (stopped) return
    const characters = Array.from(getTranslation(language, phraseKey))
    if (prefersReducedMotion) {
      selectNextPhrase()
      visibleCharacterCount = Array.from(getTranslation(language, phraseKey)).length
      phase = 'holding'
      render()
      schedule(advance, STARTUP_REDUCED_MOTION_HOLD_MS)
      return
    }
    if (phase === 'holding') {
      phase = 'fading'
      render()
      schedule(advance, STARTUP_PHRASE_FADE_MS)
      return
    }
    if (phase === 'fading') {
      selectNextPhrase()
      visibleCharacterCount = 0
      phase = 'typing'
      render()
      schedule(advance, getStartupCharacterIntervalMs(language))
      return
    }
    if (visibleCharacterCount < characters.length) {
      visibleCharacterCount += 1
      render()
      schedule(advance, getStartupCharacterIntervalMs(language))
      return
    }
    phase = 'holding'
    render()
    schedule(advance, STARTUP_PHRASE_HOLD_MS)
  }

  const handleMotionChange = (event: MediaQueryListEvent): void => {
    prefersReducedMotion = event.matches
    if (timeoutId !== null) window.clearTimeout(timeoutId)
    phase = 'holding'
    visibleCharacterCount = Array.from(getTranslation(language, phraseKey)).length
    render()
    schedule(
      advance,
      prefersReducedMotion ? STARTUP_REDUCED_MOTION_HOLD_MS : STARTUP_PHRASE_HOLD_MS
    )
  }

  render()
  schedule(advance, prefersReducedMotion ? STARTUP_REDUCED_MOTION_HOLD_MS : STARTUP_PHRASE_HOLD_MS)
  reducedMotionQuery?.addEventListener('change', handleMotionChange)

  stopActiveAnimation = () => {
    if (stopped) return
    stopped = true
    if (timeoutId !== null) window.clearTimeout(timeoutId)
    reducedMotionQuery?.removeEventListener('change', handleMotionChange)
  }
}
