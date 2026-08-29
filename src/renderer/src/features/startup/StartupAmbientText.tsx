// Renderer startup layer: cycles localized ambient phrases while blocking services hydrate.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
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

type AmbientTextPhase = 'typing' | 'holding' | 'fading'

function usePrefersReducedMotion(): boolean {
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(
    () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
  )

  useEffect(() => {
    if (!window.matchMedia) return

    const mediaQuery = window.matchMedia('(prefers-reduced-motion: reduce)')
    const handleChange = () => setPrefersReducedMotion(mediaQuery.matches)
    mediaQuery.addEventListener('change', handleChange)
    return () => mediaQuery.removeEventListener('change', handleChange)
  }, [])

  return prefersReducedMotion
}

export function StartupAmbientText() {
  const { language, t } = useFrontendConfig()
  const prefersReducedMotion = usePrefersReducedMotion()
  const characterIntervalMs = getStartupCharacterIntervalMs(language)
  const initialPhrase = t(STARTUP_BOOTSTRAP_PHRASE_KEY)
  const [phraseGroup] = useState(
    () =>
      STARTUP_AMBIENT_PHRASE_GROUPS[
        pickStartupPhraseGroupIndex(STARTUP_AMBIENT_PHRASE_GROUPS.length)
      ]
  )
  const [phraseIndex, setPhraseIndex] = useState<number | null>(null)
  const [visibleCharacterCount, setVisibleCharacterCount] = useState(
    () => Array.from(initialPhrase).length
  )
  const [phase, setPhase] = useState<AmbientTextPhase>('holding')
  const previousMotionPreference = useRef(prefersReducedMotion)
  const phrase = t(phraseIndex === null ? STARTUP_BOOTSTRAP_PHRASE_KEY : phraseGroup[phraseIndex])
  const characters = useMemo(() => Array.from(phrase), [phrase])

  useEffect(() => {
    if (previousMotionPreference.current === prefersReducedMotion) return
    previousMotionPreference.current = prefersReducedMotion

    setVisibleCharacterCount(prefersReducedMotion ? characters.length : 0)
    setPhase(prefersReducedMotion ? 'holding' : 'typing')
  }, [characters.length, prefersReducedMotion])

  useEffect(() => {
    if (prefersReducedMotion) {
      const timeoutId = window.setTimeout(() => {
        setPhraseIndex((current) =>
          current === null
            ? pickFirstStartupPhraseIndex(phraseGroup, STARTUP_BOOTSTRAP_PHRASE_KEY)
            : pickNextStartupPhraseIndex(current, phraseGroup.length)
        )
      }, STARTUP_REDUCED_MOTION_HOLD_MS)
      return () => window.clearTimeout(timeoutId)
    }

    if (phase === 'typing') {
      if (visibleCharacterCount >= characters.length) {
        setPhase('holding')
        return
      }

      const timeoutId = window.setTimeout(
        () => setVisibleCharacterCount((current) => current + 1),
        characterIntervalMs
      )
      return () => window.clearTimeout(timeoutId)
    }

    if (phase === 'holding') {
      const timeoutId = window.setTimeout(() => setPhase('fading'), STARTUP_PHRASE_HOLD_MS)
      return () => window.clearTimeout(timeoutId)
    }

    const timeoutId = window.setTimeout(() => {
      setPhraseIndex((current) =>
        current === null
          ? pickFirstStartupPhraseIndex(phraseGroup, STARTUP_BOOTSTRAP_PHRASE_KEY)
          : pickNextStartupPhraseIndex(current, phraseGroup.length)
      )
      setVisibleCharacterCount(0)
      setPhase('typing')
    }, STARTUP_PHRASE_FADE_MS)
    return () => window.clearTimeout(timeoutId)
  }, [
    characterIntervalMs,
    characters.length,
    phase,
    phraseGroup,
    prefersReducedMotion,
    visibleCharacterCount
  ])

  const visibleText = prefersReducedMotion
    ? characters.join('')
    : characters.slice(0, visibleCharacterCount).join('')

  return (
    <div className="app-startup-screen__ambient" data-phase={phase} aria-hidden="true">
      {visibleText}
    </div>
  )
}
